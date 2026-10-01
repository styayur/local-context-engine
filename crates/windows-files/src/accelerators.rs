//! Lightweight search accelerators.
//!
//! [`FileStore`] stays the single source of truth. These structures only ever
//! *propose* candidates; every proposal is verified against the real name in
//! the store before it can become a result. That separation is what makes the
//! optimisation safe: an accelerator can be stale, incomplete or conservative
//! and the answer is still exactly right.
//!
//! ```text
//!   ext:rs           -> ExtensionIndex   (posting list, already exact)
//!   pre*             -> PrefixIndex      (sorted keys, binary search)
//!   *substring*      -> TrigramIndex     (posting intersection, then verify)
//!   anything else    -> LinearFallback   (the pre-v0.2 scan)
//!
//! A text token matches a record when it appears in the **name or the path**,
//! so the trigram index is built over the normalised *full path*. Indexing only
//! names would miss every file that matches because of a parent directory, and
//! a candidate generator that is not a superset is a correctness bug — the
//! differential tests in `provider.rs` exist precisely to catch that.
//! ```
//!
//! ## Deliberate design choices
//!
//! * **No strings in the postings.** Keys are fixed-size byte arrays and ids
//!   are `u32`, so a million-entry index does not carry a million extra
//!   `String` allocations.
//! * **Append-only, id-sorted postings.** Record ids only ever grow, so
//!   appending keeps every posting list sorted without a sort pass, which is
//!   what the intersection relies on.
//! * **No removal on delete.** Deleting a record tombstones it in the store;
//!   the accelerator keeps a stale id and the verification step rejects it.
//!   Churn is handled by compaction, not by splicing every posting list.
//! * **High-frequency trigrams are dropped.** A trigram that appears in more
//!   than `max_df` records is useless for narrowing and expensive to store, so
//!   it is not indexed. Queries that need it fall back, which is slower but
//!   never wrong.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::store::FileStore;

/// Identifier of a record inside a [`FileStore`].
pub type RecordId = u32;

/// How many bytes of a normalized name the prefix index keeps.
///
/// Longer prefixes are answered by the trigram index instead, which is exact
/// because it is verified against the real name.
pub const PREFIX_KEY_LEN: usize = 24;

/// Minimum length of a query token before the trigram index is worth using.
pub const MIN_TRIGRAM_QUERY_LEN: usize = 3;

/// Fraction (as a divisor) of the corpus above which a trigram is "too common
/// to be useful".
const TRIGRAM_DF_DIVISOR: usize = 50;

/// Absolute floor for the trigram document-frequency cut-off.
const TRIGRAM_DF_FLOOR: usize = 512;

/// One entry of the prefix table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefixEntry {
    /// Normalised name bytes, right-padded.
    pub key: [u8; PREFIX_KEY_LEN],
    /// How many of `key`'s bytes are meaningful.
    pub len: u8,
    /// The record this key belongs to.
    pub id: RecordId,
}

impl PrefixEntry {
    fn new(name: &[u8], id: RecordId) -> Self {
        let mut key = [0u8; PREFIX_KEY_LEN];
        let take = name.len().min(PREFIX_KEY_LEN);
        key[..take].copy_from_slice(&name[..take]);
        Self {
            key,
            len: u8::try_from(take).unwrap_or(u8::MAX),
            id,
        }
    }

    /// The meaningful bytes of the key.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.key[..usize::from(self.len)]
    }

    /// Whether this entry starts with `prefix`.
    #[must_use]
    pub fn starts_with(&self, prefix: &[u8]) -> bool {
        self.bytes().starts_with(prefix)
    }
}

/// `extension -> sorted record ids`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtensionIndex {
    postings: BTreeMap<String, Vec<RecordId>>,
    indexed: usize,
}

impl ExtensionIndex {
    /// Insert one record's extension.
    pub fn insert(&mut self, extension: &str, id: RecordId) {
        if extension.is_empty() {
            return;
        }
        let key = extension.to_ascii_lowercase();
        let list = self.postings.entry(key).or_default();
        if list.last() != Some(&id) {
            list.push(id);
        }
        self.indexed += 1;
    }

    /// Every record with this extension.
    #[must_use]
    pub fn lookup(&self, extension: &str) -> &[RecordId] {
        let key = extension.trim_start_matches('.').to_ascii_lowercase();
        self.postings.get(&key).map_or(&[], Vec::as_slice)
    }

    /// Every record whose extension is one of `extensions`.
    #[must_use]
    pub fn lookup_any(&self, extensions: &[String]) -> Vec<RecordId> {
        let mut out: Vec<RecordId> = Vec::new();
        for extension in extensions {
            out.extend_from_slice(self.lookup(extension));
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// How many distinct extensions are indexed.
    #[must_use]
    pub fn distinct(&self) -> usize {
        self.postings.len()
    }

    /// Total postings stored.
    #[must_use]
    pub fn postings(&self) -> usize {
        self.postings.values().map(Vec::len).sum()
    }

    /// Approximate heap footprint in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.indexed * std::mem::size_of::<RecordId>()
            + self.postings.keys().map(String::len).sum::<usize>()
    }
}

/// Sorted table of normalised name prefixes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrefixIndex {
    entries: Vec<PrefixEntry>,
}

impl PrefixIndex {
    /// Insert one record's name.
    pub fn insert(&mut self, normalised_name: &[u8], id: RecordId) {
        if normalised_name.is_empty() {
            return;
        }
        // Ids only grow, so pushing keeps `entries` sorted by key for a fresh
        // build; `finish` restores the invariant after a batch.
        self.entries.push(PrefixEntry::new(normalised_name, id));
    }

    /// Restore the sorted-by-key invariant. Call once after a build batch.
    pub fn finish(&mut self) {
        self.entries.sort_unstable_by(|left, right| {
            left.bytes()
                .cmp(right.bytes())
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    /// Every record whose normalised name starts with `prefix`.
    ///
    /// The prefix is normalised here rather than by the caller, because a
    /// forgotten `to_lowercase` would silently return nothing.
    #[must_use]
    pub fn lookup(&self, prefix: &str) -> &[PrefixEntry] {
        let normalised = normalise_key(prefix);
        self.lookup_normalised(&normalised)
    }

    /// Lookup for callers that already hold normalised bytes.
    #[must_use]
    pub fn lookup_normalised(&self, prefix: &[u8]) -> &[PrefixEntry] {
        if prefix.is_empty() || prefix.len() > PREFIX_KEY_LEN {
            return &[];
        }
        // Entries are sorted by key, so "starts with" is monotone after the
        // first entry that is not below the prefix: matches form one run.
        let start = self.entries.partition_point(|entry| entry.bytes() < prefix);
        let tail = &self.entries[start..];
        let count = tail.partition_point(|entry| entry.starts_with(prefix));
        &tail[..count]
    }

    /// How many entries are stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Approximate heap footprint in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.entries.len() * std::mem::size_of::<PrefixEntry>()
    }
}

/// `trigram -> sorted record ids`, with common trigrams dropped.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrigramIndex {
    postings: HashMap<[u8; 3], Vec<RecordId>>,
    /// Trigrams that were seen but are too common to be worth storing.
    ///
    /// This set is kept forever, not just during construction. A single live
    /// insert must never resurrect a dropped trigram: doing so would create a
    /// posting list of length one for a trigram that appears in half the
    /// corpus, and intersecting that tiny list would silently *lose* matches.
    dropped_trigrams: HashSet<[u8; 3]>,
    /// How many records contributed.
    documents: usize,
}

impl TrigramIndex {
    /// The document-frequency cut-off for a corpus of `documents` records.
    #[must_use]
    pub fn max_df(documents: usize) -> usize {
        (documents / TRIGRAM_DF_DIVISOR).max(TRIGRAM_DF_FLOOR)
    }

    /// Insert one record's normalised name.
    pub fn insert(&mut self, normalised_name: &[u8], id: RecordId) {
        if normalised_name.len() < MIN_TRIGRAM_QUERY_LEN {
            return;
        }
        // No per-document set: repeated trigrams collapse through the
        // last-id check, and `finish` deduplicates whatever survives.
        for window in normalised_name.windows(3) {
            let trigram: [u8; 3] = [window[0], window[1], window[2]];
            if self.dropped_trigrams.contains(&trigram) {
                // Too common to index. Adding it here would create a list that
                // is not a superset of the true matches.
                continue;
            }
            let list = self.postings.entry(trigram).or_default();
            if list.last() != Some(&id) {
                list.push(id);
            }
        }
    }

    /// Drop postings that are too common to be useful, then sort what is left.
    ///
    /// Must be called once a build batch is complete.
    pub fn finish(&mut self, documents: usize) {
        self.documents = documents;
        let max_df = Self::max_df(documents);
        let mut dropped: Vec<[u8; 3]> = Vec::new();
        self.postings.retain(|trigram, list| {
            list.sort_unstable();
            list.dedup();
            if list.len() > max_df {
                dropped.push(*trigram);
                return false;
            }
            true
        });
        self.dropped_trigrams.extend(dropped);
        for list in self.postings.values_mut() {
            list.shrink_to_fit();
        }
    }

    /// Postings for one trigram, or `None` when it was dropped or never seen.
    #[must_use]
    pub fn lookup(&self, trigram: &[u8; 3]) -> Option<&[RecordId]> {
        self.postings.get(trigram).map(Vec::as_slice)
    }

    /// The distinct trigrams of a query token.
    #[must_use]
    pub fn trigrams_of(token: &[u8]) -> Vec<[u8; 3]> {
        let mut out: Vec<[u8; 3]> = Vec::new();
        if token.len() < MIN_TRIGRAM_QUERY_LEN {
            return out;
        }
        for window in token.windows(3) {
            let trigram = [window[0], window[1], window[2]];
            if !out.contains(&trigram) {
                out.push(trigram);
            }
        }
        out
    }

    /// Intersect the posting lists of `trigrams`, smallest first.
    ///
    /// Returns `None` when no trigram of the query is indexed, which means the
    /// caller must fall back rather than conclude "no matches": an unindexed
    /// trigram is either too common or absent, and only verification can tell
    /// those apart.
    #[must_use]
    pub fn intersect(&self, trigrams: &[[u8; 3]]) -> Option<Vec<RecordId>> {
        let mut lists: Vec<&[RecordId]> = trigrams
            .iter()
            .filter_map(|trigram| self.lookup(trigram))
            .collect();
        if lists.is_empty() {
            return None;
        }
        // Rarest first: the intersection shrinks fastest that way.
        lists.sort_unstable_by_key(|list| list.len());

        let mut current: Vec<RecordId> = lists[0].to_vec();
        for list in &lists[1..] {
            if current.is_empty() {
                break;
            }
            current = intersect_sorted(&current, list);
        }
        Some(current)
    }

    /// How many distinct trigrams are stored.
    #[must_use]
    pub fn distinct(&self) -> usize {
        self.postings.len()
    }

    /// How many trigrams were dropped for being too common.
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.dropped_trigrams.len()
    }

    /// Total postings stored.
    #[must_use]
    pub fn postings(&self) -> usize {
        self.postings.values().map(Vec::len).sum()
    }

    /// Approximate heap footprint in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        let headers = (self.postings.len() + self.dropped_trigrams.len())
            * (std::mem::size_of::<[u8; 3]>() + 24);
        let ids = self.postings() * std::mem::size_of::<RecordId>();
        headers + ids
    }
}

/// Intersect two ascending, deduplicated lists.
#[must_use]
pub fn intersect_sorted(left: &[RecordId], right: &[RecordId]) -> Vec<RecordId> {
    let mut out = Vec::with_capacity(left.len().min(right.len()));
    let (mut i, mut j) = (0usize, 0usize);
    while i < left.len() && j < right.len() {
        match left[i].cmp(&right[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(left[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// What the accelerators cost, for the index panel and the benchmark docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceleratorStats {
    /// Records the accelerators were built for.
    pub documents: usize,
    /// Distinct extensions.
    pub extensions: usize,
    /// Prefix table entries.
    pub prefixes: usize,
    /// Distinct trigrams stored.
    pub trigrams: usize,
    /// Trigrams dropped for being too common.
    pub trigrams_dropped: usize,
    /// Total trigram postings.
    pub trigram_postings: usize,
    /// Approximate heap footprint of all three structures.
    pub memory_bytes: usize,
}

/// The three accelerators, kept in sync with a [`FileStore`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchAccelerators {
    extension: ExtensionIndex,
    prefix: PrefixIndex,
    trigram: TrigramIndex,
    /// Records added since the last full build, for compaction policy.
    dirty: usize,
}

impl SearchAccelerators {
    /// Build every accelerator for an existing store.
    ///
    /// Paths are cached while walking the store, which costs one transient
    /// `String` per record during the build. That is a deliberate trade: the
    /// alternative is walking the parent chain for every record, which is
    /// `O(entries × depth)` string work on a million entry volume.
    #[must_use]
    pub fn build(store: &FileStore) -> Self {
        let mut accelerators = Self::default();
        let mut paths: Vec<String> = Vec::with_capacity(store.len());

        for (offset, record) in store.records().iter().enumerate() {
            let Ok(id) = RecordId::try_from(offset) else {
                break;
            };
            if record.is_deleted() {
                paths.push(String::new());
                continue;
            }
            let name = store.name(id);
            if name.is_empty() {
                paths.push(String::new());
                continue;
            }

            let path = match record.parent {
                crate::store::NO_PARENT => name.to_string(),
                parent => match paths.get(parent as usize) {
                    Some(parent_path) if !parent_path.is_empty() => {
                        if parent_path.ends_with('\\') {
                            format!("{parent_path}{name}")
                        } else {
                            format!("{parent_path}\\{name}")
                        }
                    }
                    // Parent not cached yet (out-of-order store): fall back to
                    // the authoritative walk.
                    _ => store.path_of(id),
                },
            };

            accelerators
                .extension
                .insert(store.extension(id).unwrap_or(""), id);
            accelerators.prefix.insert(&normalise_key(name), id);
            accelerators.trigram.insert(&normalise_key(&path), id);
            accelerators.dirty += 1;
            paths.push(path);
        }

        accelerators.finish(store.len());
        accelerators
    }

    /// Sort and prune the structures once a build batch is complete.
    pub fn finish(&mut self, documents: usize) {
        self.prefix.finish();
        self.trigram.finish(documents);
        self.dirty = 0;
    }

    /// Index one record, or re-index it after a rename.
    ///
    /// Stale postings are intentionally left behind: verification against the
    /// store discards them, and compaction removes them in bulk.
    pub fn insert(&mut self, store: &FileStore, id: RecordId) {
        let name = store.name(id);
        if name.is_empty() {
            return;
        }
        let extension = store.extension(id).unwrap_or("").to_string();
        let path = store.path_of(id);
        self.insert_indexed(&extension, name, &path, id);
    }

    /// Index one record without holding a borrow on the store.
    ///
    /// The caller passes owned strings so a write lock can update the store and
    /// the accelerators in the same critical section.
    pub fn insert_indexed(&mut self, extension: &str, name: &str, path: &str, id: RecordId) {
        if name.is_empty() {
            return;
        }
        self.extension.insert(extension, id);
        // Postings are sorted by id; appending a higher id keeps that.
        self.prefix
            .entries
            .push(PrefixEntry::new(&normalise_key(name), id));
        self.trigram.insert(&normalise_key(path), id);
        self.dirty += 1;
    }

    /// Note a batch of mutations so the compaction policy can see them.
    pub fn note_mutations(&mut self, applied: usize) {
        self.dirty += applied;
    }

    /// Whether the accelerators have drifted far enough to be worth rebuilding.
    #[must_use]
    pub fn needs_compaction(&self, store_len: usize) -> bool {
        if store_len == 0 {
            return false;
        }
        // 5% churn, or 20 000 records, whichever is smaller.
        let threshold = (store_len / 20).clamp(1_000, 20_000);
        self.dirty >= threshold
    }

    /// The accumulated drift since the last full build.
    #[must_use]
    pub fn dirty(&self) -> usize {
        self.dirty
    }

    /// Extension postings.
    #[must_use]
    pub fn extensions(&self) -> &ExtensionIndex {
        &self.extension
    }

    /// Prefix table.
    #[must_use]
    pub fn prefixes(&self) -> &PrefixIndex {
        &self.prefix
    }

    /// Trigram postings.
    #[must_use]
    pub fn trigrams(&self) -> &TrigramIndex {
        &self.trigram
    }

    /// Summary for the status panel and the docs.
    #[must_use]
    pub fn stats(&self) -> AcceleratorStats {
        AcceleratorStats {
            documents: self.prefix.len(),
            extensions: self.extension.distinct(),
            prefixes: self.prefix.len(),
            trigrams: self.trigram.distinct(),
            trigrams_dropped: self.trigram.dropped(),
            trigram_postings: self.trigram.postings(),
            memory_bytes: self.memory_bytes(),
        }
    }

    /// Approximate heap footprint in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.extension.memory_bytes() + self.prefix.memory_bytes() + self.trigram.memory_bytes()
    }
}

/// Lower-case a name into the byte form the accelerators key on.
///
/// The mapping is one character to one character, so byte offsets never shift
/// relative to the caller's expectations and a Chinese name is lower-cased
/// without being re-encoded.
#[must_use]
pub fn normalise_key(name: &str) -> Vec<u8> {
    let mut out = String::with_capacity(name.len());
    for character in name.chars() {
        let mut lowered = character.to_lowercase();
        match (lowered.next(), lowered.next()) {
            (Some(single), None) => out.push(single),
            _ => out.push(character),
        }
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(names: &[&str]) -> FileStore {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        for name in names {
            store.push_entry(root, name, 'C', false, 1, 0, 0, 0);
        }
        store
    }

    #[test]
    fn normalisation_is_one_char_to_one_char() {
        assert_eq!(normalise_key("README.MD"), b"readme.md");
        assert_eq!(normalise_key("ÜBERSICHT"), "übersicht".as_bytes());
        assert_eq!(normalise_key("年度报告.PDF"), "年度报告.pdf".as_bytes());
    }

    #[test]
    fn prefix_keys_are_truncated_but_keep_their_length() {
        let long = "a".repeat(64);
        let entry = PrefixEntry::new(long.as_bytes(), 7);
        assert_eq!(entry.bytes().len(), PREFIX_KEY_LEN);
        assert_eq!(entry.id, 7);
        assert!(entry.starts_with(b"aaa"));
    }

    #[test]
    fn the_extension_index_groups_by_extension() {
        let store = store_with(&["a.rs", "b.rs", "c.toml"]);
        let accelerators = SearchAccelerators::build(&store);
        let rs = accelerators.extensions().lookup("rs");
        assert_eq!(rs.len(), 2);
        assert_eq!(rs, &[1, 2]);
        assert_eq!(accelerators.extensions().lookup(".RS").len(), 2);
        assert_eq!(accelerators.extensions().lookup("toml"), &[3]);
        assert!(accelerators.extensions().lookup("nope").is_empty());
    }

    #[test]
    fn the_prefix_index_finds_a_sorted_range() {
        let store = store_with(&["alpha.txt", "album.txt", "beta.txt", "al"]);
        let accelerators = SearchAccelerators::build(&store);
        let hits = accelerators.prefixes().lookup("al");
        let ids: Vec<RecordId> = hits.iter().map(|entry| entry.id).collect();
        assert_eq!(ids.len(), 3, "{ids:?}");
        assert!(hits.iter().all(|entry| entry.starts_with(b"al")));
    }

    #[test]
    fn the_prefix_index_is_case_insensitive() {
        let store = store_with(&["ReadMe.md"]);
        let accelerators = SearchAccelerators::build(&store);
        assert_eq!(accelerators.prefixes().lookup("readme").len(), 1);
        assert_eq!(accelerators.prefixes().lookup("README").len(), 1);
    }

    #[test]
    fn a_prefix_longer_than_the_key_is_not_answered_from_the_table() {
        let store = store_with(&["short.txt"]);
        let accelerators = SearchAccelerators::build(&store);
        let too_long = vec![b'a'; PREFIX_KEY_LEN + 1];
        assert!(accelerators
            .prefixes()
            .lookup_normalised(&too_long)
            .is_empty());
    }

    #[test]
    fn trigrams_of_a_token_are_distinct_and_ordered() {
        assert_eq!(
            TrigramIndex::trigrams_of(b"visual"),
            vec![*b"vis", *b"isu", *b"sua", *b"ual"]
        );
        assert!(TrigramIndex::trigrams_of(b"ab").is_empty());
    }

    #[test]
    fn trigrams_found_in_a_name_are_lookup_able() {
        let store = store_with(&["visual-studio-code.exe"]);
        let accelerators = SearchAccelerators::build(&store);
        let trigrams = TrigramIndex::trigrams_of(b"studio");
        let candidates = accelerators
            .trigrams()
            .intersect(&trigrams)
            .expect("`studio` occurs, so at least one trigram is indexed");
        assert!(candidates.contains(&1));
    }

    #[test]
    fn an_intersection_never_contains_a_record_missing_a_trigram() {
        let store = store_with(&["alpha.rs", "beta.rs", "gamma.rs"]);
        let accelerators = SearchAccelerators::build(&store);
        let trigrams = TrigramIndex::trigrams_of(b"eta");
        let candidates = accelerators.trigrams().intersect(&trigrams).unwrap();
        // Only `beta.rs` contains `eta`.
        assert_eq!(candidates, vec![2]);
    }

    #[test]
    fn common_trigrams_are_dropped_and_reported() {
        // Build a corpus where every name shares the string "common".
        let names: Vec<String> = (0..4_000).map(|i| format!("common-{i}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let store = store_with(&refs);
        let accelerators = SearchAccelerators::build(&store);
        let stats = accelerators.stats();
        assert!(
            stats.trigrams_dropped > 0,
            "`com` and friends appear in every record and must be dropped"
        );
        // A dropped trigram is simply absent, and `intersect` reports that.
        assert!(accelerators.trigrams().lookup(b"com").is_none());
    }

    #[test]
    fn intersect_reports_none_when_no_trigram_is_indexed() {
        let store = store_with(&["a.txt"]);
        let accelerators = SearchAccelerators::build(&store);
        let trigrams = TrigramIndex::trigrams_of(b"zzz");
        assert!(accelerators.trigrams().intersect(&trigrams).is_none());
    }

    #[test]
    fn sorted_intersection_matches_the_naive_one() {
        let left = [1u32, 3, 5, 7, 9, 11];
        let right = [3u32, 4, 5, 9, 12];
        assert_eq!(intersect_sorted(&left, &right), vec![3, 5, 9]);
        assert!(intersect_sorted(&[], &right).is_empty());
        assert!(intersect_sorted(&left, &[]).is_empty());
    }

    #[test]
    fn a_live_insert_never_resurrects_a_dropped_trigram() {
        let names: Vec<String> = (0..4_000).map(|i| format!("common-{i}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut store = store_with(&refs);
        let mut accelerators = SearchAccelerators::build(&store);
        assert!(accelerators.trigrams().lookup(b"com").is_none());

        let root = store.root_index('C').unwrap();
        let id = store.push_entry(root, "common-late.txt", 'C', false, 1, 0, 0, 0);
        accelerators.insert(&store, id);

        // Re-adding the id must not create a one-element list for a trigram
        // that appears in four thousand other records.
        assert!(
            accelerators.trigrams().lookup(b"com").is_none(),
            "a dropped trigram must stay dropped until compaction"
        );
        // The record is still reachable through its other, rarer trigrams.
        assert!(accelerators
            .trigrams()
            .intersect(&TrigramIndex::trigrams_of(b"late"))
            .is_some_and(|ids| ids.contains(&id)));
    }

    #[test]
    fn a_live_insert_is_visible_without_a_rebuild() {
        let mut store = store_with(&["first.txt"]);
        let mut accelerators = SearchAccelerators::build(&store);
        let root = store.root_index('C').unwrap();
        let id = store.push_entry(root, "second.pdf", 'C', false, 1, 0, 0, 0);
        accelerators.insert(&store, id);

        assert_eq!(accelerators.extensions().lookup("pdf"), &[id]);
        assert_eq!(accelerators.prefixes().lookup("second").len(), 1);
        assert!(accelerators
            .trigrams()
            .intersect(&TrigramIndex::trigrams_of(b"cond"))
            .is_some_and(|ids| ids.contains(&id)));
    }

    #[test]
    fn a_crossed_compaction_threshold_is_reported() {
        let store = store_with(&["a.txt"]);
        let mut accelerators = SearchAccelerators::build(&store);
        assert!(!accelerators.needs_compaction(1_000));
        accelerators.note_mutations(20_000);
        assert!(accelerators.needs_compaction(1_000_000));
        assert_eq!(accelerators.dirty(), 20_000);
    }

    #[test]
    fn deleted_records_are_not_indexed_when_building() {
        let mut store = store_with(&["keep.txt", "gone.txt"]);
        store.mark_deleted(2);
        let accelerators = SearchAccelerators::build(&store);
        assert_eq!(accelerators.extensions().lookup("txt"), &[1]);
        assert!(accelerators.prefixes().lookup("gone").is_empty());
    }

    #[test]
    fn stats_describe_every_structure() {
        let store = store_with(&["alpha.rs", "beta.rs", "gamma.toml"]);
        let accelerators = SearchAccelerators::build(&store);
        let stats = accelerators.stats();
        // Four records: the volume root plus three files. The root is indexed
        // like anything else, which is what keeps the accelerated path and the
        // linear path in agreement.
        assert_eq!(stats.documents, 4);
        assert_eq!(stats.extensions, 2);
        assert_eq!(stats.prefixes, 4);
        assert!(stats.trigrams > 0);
        assert!(stats.memory_bytes > 0);
    }

    #[test]
    fn accelerators_round_trip_through_messagepack() {
        let store = store_with(&["alpha.rs", "beta.toml"]);
        let accelerators = SearchAccelerators::build(&store);
        let encoded = rmp_serde::to_vec_named(&accelerators).unwrap();
        let decoded: SearchAccelerators = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(decoded.extensions().lookup("rs"), &[1]);
        assert_eq!(decoded.prefixes().lookup("beta").len(), 1);
        assert_eq!(decoded.stats().documents, 3);
    }

    #[test]
    fn unicode_names_are_indexed_and_found() {
        let store = store_with(&["年度报告.pdf", "季度总结.pdf"]);
        let accelerators = SearchAccelerators::build(&store);
        assert_eq!(accelerators.extensions().lookup("pdf").len(), 2);
        let trigrams = TrigramIndex::trigrams_of("年度报".as_bytes());
        assert!(accelerators.trigrams().intersect(&trigrams).is_some());
    }
}
