//! The query planner.
//!
//! Every query used to take the same path: walk the whole index, test each
//! record, rank what survives. That is fine at 10 000 entries and painful at a
//! million when the query is `zzqxjw`.
//!
//! The planner picks the cheapest *correct* candidate source instead:
//!
//! | query shape                | source              | exact? |
//! |----------------------------|---------------------|--------|
//! | `ext:rs`                   | extension postings  | yes    |
//! | `prefix:vscode`            | prefix table        | superset |
//! | a token of 3+ characters   | trigram postings    | superset |
//! | two of the above           | intersection        | superset |
//! | anything shorter or absent | linear scan         | yes    |
//!
//! "Superset" is the load-bearing word. Accelerated sources only ever *propose*
//! candidates; the provider verifies every one of them against the real name in
//! the [`FileStore`]. Stale extra IDs are safe; missing live IDs are not.
//! Dropped trigrams are tracked explicitly and mutations must publish the
//! store and accelerators together. Differential tests enforce this contract.

use std::time::Instant;

use search_core::{Filter, SearchQuery};
use serde::{Deserialize, Serialize};

use crate::accelerators::{
    intersect_sorted, normalise_key, RecordId, SearchAccelerators, TrigramIndex,
    MIN_TRIGRAM_QUERY_LEN,
};
use crate::store::FileStore;

/// Upper bound on the candidate list the planner is willing to materialise.
///
/// Past this point a linear scan streams better than a Vec of ids, so the
/// planner switches plans rather than allocating.
pub const MAX_PLANNED_CANDIDATES: usize = 250_000;

/// Which algorithm answered a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SearchPlan {
    /// Answered entirely from the extension postings.
    ExtensionLookup,
    /// Proposed by the prefix table, then verified against the full name.
    PrefixLookup,
    /// Answered from trigram postings, verified against real names.
    TrigramLookup,
    /// Two or more sources intersected.
    CandidateIntersection,
    /// The pre-v0.2 full scan.
    LinearFallback,
}

impl SearchPlan {
    /// Lower-case wire name used by `--explain`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SearchPlan::ExtensionLookup => "extension",
            SearchPlan::PrefixLookup => "prefix",
            SearchPlan::TrigramLookup => "trigram",
            SearchPlan::CandidateIntersection => "intersection",
            SearchPlan::LinearFallback => "linear",
        }
    }

    /// Whether the source is provably exact rather than a superset.
    #[must_use]
    pub const fn is_exact(self) -> bool {
        matches!(self, SearchPlan::ExtensionLookup)
    }
}

impl std::fmt::Display for SearchPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One candidate source and how many ids it produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanSource {
    /// Source name, for example `trigram(studio)`.
    pub name: String,
    /// Ids the source produced before intersection.
    pub candidates: usize,
}

/// What the planner did, for `--explain` and for the benchmarks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryPlanInfo {
    /// The chosen plan.
    pub plan: SearchPlan,
    /// Sources that contributed candidates.
    pub sources: Vec<PlanSource>,
    /// Candidates handed to verification.
    pub initial_candidates: usize,
    /// Candidates that survived structured filters.
    pub after_filters: usize,
    /// Candidates that survived verification and were ranked.
    pub verified: usize,
    /// Whether the candidate list was cut short.
    pub truncated: bool,
    /// Planning time in milliseconds.
    pub elapsed_ms: f64,
    /// Anything the user should know about the plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl QueryPlanInfo {
    fn new(plan: SearchPlan) -> Self {
        Self {
            plan,
            sources: Vec::new(),
            initial_candidates: 0,
            after_filters: 0,
            verified: 0,
            truncated: false,
            elapsed_ms: 0.0,
            note: None,
        }
    }

    /// One line per source, as `--explain` prints them.
    #[must_use]
    pub fn describe_sources(&self) -> String {
        self.sources
            .iter()
            .map(|source| format!("{}={}", source.name, source.candidates))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Where the provider should read candidates from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateSource {
    /// Use `PlannedCandidates::ids`.
    Ids,
    /// Walk the store in record order, as v0.1 did.
    LinearScan,
}

/// The planner's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedCandidates {
    /// Candidate ids, ascending and deduplicated. Empty for a linear scan.
    pub ids: Vec<RecordId>,
    /// How the provider should read candidates.
    pub source: CandidateSource,
    /// What the planner did.
    pub info: QueryPlanInfo,
}

impl PlannedCandidates {
    fn linear(info: QueryPlanInfo) -> Self {
        Self {
            ids: Vec::new(),
            source: CandidateSource::LinearScan,
            info,
        }
    }
}

/// Plan a query against a store and its accelerators.
#[must_use]
pub fn plan(
    store: &FileStore,
    accelerators: &SearchAccelerators,
    query: &SearchQuery,
) -> PlannedCandidates {
    let started = Instant::now();
    let mut info = QueryPlanInfo::new(SearchPlan::LinearFallback);

    // Predicates are conjunctive. An unindexable predicate is left to the
    // authoritative verifier; it must not discard other selective sources.
    let mut sources: Vec<(String, Candidate<'_>)> = Vec::new();
    for filter in &query.filters {
        match filter {
            Filter::Extension(ext) if !ext.trim_start_matches('.').is_empty() => {
                sources.push((
                    format!("extension({ext})"),
                    Candidate::Posting(accelerators.extensions().lookup(ext)),
                ));
            }
            Filter::NamePrefix(prefix) if !prefix.is_empty() => {
                sources.push((
                    format!("prefix({prefix})"),
                    Candidate::Prefix(accelerators.prefixes().lookup(prefix)),
                ));
            }
            // Path filters and name filters share the full-path trigram superset.
            Filter::Path(text) | Filter::Name(text) => {
                add_trigrams(&mut sources, accelerators, text);
            }
            _ => {}
        }
    }
    for token in query.text.as_deref().unwrap_or("").split_whitespace() {
        add_trigrams(&mut sources, accelerators, token);
    }
    sources.sort_by_key(|(_, source)| source.len());
    if sources.is_empty() || sources[0].1.len() > MAX_PLANNED_CANDIDATES {
        info.note = Some("no selective indexed predicate within the candidate budget".into());
        info.initial_candidates = store.len();
        info.elapsed_ms = elapsed_ms(started);
        return PlannedCandidates::linear(info);
    }
    let (name, source) = sources.remove(0);
    info.plan = match &source {
        Candidate::Prefix(_) => SearchPlan::PrefixLookup,
        Candidate::Posting(_) => SearchPlan::ExtensionLookup,
        Candidate::Trigrams(_) => SearchPlan::TrigramLookup,
    };
    let mut ids = source.ids();
    info.sources.push(PlanSource {
        name,
        candidates: ids.len(),
    });
    for (name, source) in sources {
        if ids.is_empty() {
            break;
        }
        // Materializing a broad source costs more than verifying a small seed.
        // Cardinality is a cheap cost estimate; skipping is always conservative.
        if source.len() > ids.len().saturating_mul(8) {
            continue;
        }
        let next = source.ids();
        info.sources.push(PlanSource {
            name,
            candidates: next.len(),
        });
        ids = intersect_sorted(&ids, &next);
        if !matches!(source, Candidate::Trigrams(_)) || info.plan != SearchPlan::TrigramLookup {
            info.plan = SearchPlan::CandidateIntersection;
        }
    }
    info.initial_candidates = ids.len();
    info.elapsed_ms = elapsed_ms(started);
    PlannedCandidates {
        ids,
        source: CandidateSource::Ids,
        info,
    }
}

// Borrow large postings until the cost estimate selects them.
enum Candidate<'a> {
    Posting(&'a [RecordId]),
    Prefix(&'a [crate::accelerators::PrefixEntry]),
    Trigrams(Vec<RecordId>),
}

impl Candidate<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Posting(ids) => ids.len(),
            Self::Prefix(entries) => entries.len(),
            Self::Trigrams(ids) => ids.len(),
        }
    }
    fn ids(&self) -> Vec<RecordId> {
        match self {
            Self::Posting(ids) => ids.to_vec(),
            Self::Trigrams(ids) => ids.clone(),
            Self::Prefix(entries) => {
                let mut ids: Vec<_> = entries.iter().map(|entry| entry.id).collect();
                ids.sort_unstable();
                ids.dedup();
                ids
            }
        }
    }
}

fn add_trigrams<'a>(
    sources: &mut Vec<(String, Candidate<'a>)>,
    accelerators: &SearchAccelerators,
    text: &str,
) {
    let key = normalise_key(text);
    if key.len() < MIN_TRIGRAM_QUERY_LEN {
        return;
    }
    if let Some(ids) = accelerators
        .trigrams()
        .intersect(&TrigramIndex::trigrams_of(&key))
    {
        sources.push((format!("trigram({text})"), Candidate::Trigrams(ids)));
    }
}

/// A plan for a query that is already known to be a plain linear scan.
#[must_use]
pub fn linear_plan(note: impl Into<String>) -> QueryPlanInfo {
    let mut info = QueryPlanInfo::new(SearchPlan::LinearFallback);
    info.note = Some(note.into());
    info
}

fn elapsed_ms(started: Instant) -> f64 {
    let micros = started.elapsed().as_micros() as f64;
    (micros / 1_000.0 * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::FileStore;

    fn store_with(names: &[&str]) -> FileStore {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        for name in names {
            let extension = if name.contains('.') {
                crate::store::extension_of(name)
            } else {
                None
            };
            store.push_entry(root, name, 'C', extension.is_none(), 1, 0, 0, 0);
        }
        store
    }

    fn planned(
        store: &FileStore,
        accelerators: &SearchAccelerators,
        dsl: &str,
    ) -> PlannedCandidates {
        let query = query_dsl::parse(dsl).query;
        plan(store, accelerators, &query)
    }

    #[test]
    fn an_extension_only_query_uses_the_extension_index() {
        let store = store_with(&["a.rs", "b.txt", "c.rs"]);
        let accelerators = SearchAccelerators::build(&store);
        let result = planned(&store, &accelerators, "type:file ext:rs");
        assert_eq!(result.info.plan, SearchPlan::ExtensionLookup);
        assert_eq!(result.source, CandidateSource::Ids);
        assert_eq!(result.ids, vec![1, 3]);
        assert!(result.info.plan.is_exact());
    }

    #[test]
    fn a_three_character_token_uses_the_trigram_index() {
        let store = store_with(&["visual-studio-code.exe", "notes.txt"]);
        let accelerators = SearchAccelerators::build(&store);
        let result = planned(&store, &accelerators, "studio");
        assert_eq!(result.info.plan, SearchPlan::TrigramLookup);
        assert!(result.ids.contains(&1));
        assert!(!result.ids.contains(&2));
    }

    #[test]
    fn a_two_character_token_falls_back_to_a_linear_scan() {
        let store = store_with(&["alpha.rs"]);
        let accelerators = SearchAccelerators::build(&store);
        let result = planned(&store, &accelerators, "al");
        assert_eq!(result.info.plan, SearchPlan::LinearFallback);
        assert_eq!(result.source, CandidateSource::LinearScan);
        assert!(result.info.note.is_some());
    }

    #[test]
    fn a_prefix_filter_uses_the_prefix_table() {
        let store = store_with(&["vscode.exe", "code.exe", "notes.txt"]);
        let accelerators = SearchAccelerators::build(&store);
        let result = planned(&store, &accelerators, "prefix:vscode");
        assert_eq!(result.info.plan, SearchPlan::PrefixLookup);
        assert_eq!(result.ids, vec![1]);
        assert!(!result.info.plan.is_exact());
    }

    #[test]
    fn an_extension_and_a_token_are_intersected() {
        let store = store_with(&["alpha.rs", "alpha.txt", "beta.rs"]);
        let accelerators = SearchAccelerators::build(&store);
        let result = planned(&store, &accelerators, "type:file ext:rs alpha");
        assert_eq!(result.info.plan, SearchPlan::CandidateIntersection);
        assert_eq!(result.ids, vec![1]);
        assert_eq!(result.info.sources.len(), 2);
    }

    #[test]
    fn a_path_filter_keeps_usable_accelerators() {
        let store = store_with(&["alpha.rs"]);
        let accelerators = SearchAccelerators::build(&store);
        let result = planned(&store, &accelerators, r"path:projects alpha");
        assert_eq!(result.source, CandidateSource::Ids);
        assert!(result.ids.is_empty());
    }

    #[test]
    fn a_query_with_nothing_indexable_scans() {
        let store = store_with(&["alpha.rs"]);
        let accelerators = SearchAccelerators::build(&store);
        let result = planned(&store, &accelerators, "type:file");
        assert_eq!(result.info.plan, SearchPlan::LinearFallback);
    }

    #[test]
    fn multiple_tokens_are_intersected() {
        let store = store_with(&["visual-studio-code.exe", "visual-basic.exe", "studio.txt"]);
        let accelerators = SearchAccelerators::build(&store);
        let result = planned(&store, &accelerators, "visual code");
        assert_eq!(result.info.plan, SearchPlan::TrigramLookup);
        assert_eq!(result.ids, vec![1]);
    }

    #[test]
    fn an_absent_token_produces_no_candidates_rather_than_a_scan() {
        let store = store_with(&["alpha.rs"]);
        let accelerators = SearchAccelerators::build(&store);
        let result = planned(&store, &accelerators, "zzzqxjw");
        assert_eq!(result.info.plan, SearchPlan::TrigramLookup);
        assert!(result.ids.is_empty());
    }

    #[test]
    fn planning_is_microseconds_for_a_large_index() {
        let names: Vec<String> = (0..20_000).map(|i| format!("file-{i}.rs")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let store = store_with(&refs);
        let accelerators = SearchAccelerators::build(&store);
        let result = planned(&store, &accelerators, "file-19999");
        assert_eq!(result.info.plan, SearchPlan::TrigramLookup);
        // Trigrams are a filter, not the answer: the query shares every
        // trigram with its neighbours `file-19998`, `file-1999x` and so on, so
        // a handful of candidates is the expected, correct outcome. The point
        // is that it is a handful rather than twenty thousand.
        assert!(
            result.ids.len() < 100,
            "expected a small candidate set, got {}",
            result.ids.len()
        );
        assert!(result.info.elapsed_ms < 50.0);
    }

    #[test]
    fn the_info_renders_its_sources() {
        let store = store_with(&["alpha.rs", "beta.rs"]);
        let accelerators = SearchAccelerators::build(&store);
        let result = planned(&store, &accelerators, "type:file ext:rs alpha");
        let described = result.info.describe_sources();
        assert!(described.contains("extension"));
        assert!(described.contains("trigram"));
    }

    #[test]
    fn a_linear_plan_carries_its_explanation() {
        let info = linear_plan("because");
        assert_eq!(info.plan, SearchPlan::LinearFallback);
        assert_eq!(info.note.as_deref(), Some("because"));
    }

    #[test]
    fn plan_names_are_stable_wire_values() {
        for plan in [
            SearchPlan::ExtensionLookup,
            SearchPlan::PrefixLookup,
            SearchPlan::TrigramLookup,
            SearchPlan::CandidateIntersection,
            SearchPlan::LinearFallback,
        ] {
            let json = serde_json::to_string(&plan).unwrap();
            assert!(json.contains(plan.as_str()), "{json}");
        }
    }
}
