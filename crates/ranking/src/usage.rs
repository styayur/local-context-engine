//! Tiny local usage history.
//!
//! The engine remembers *which result you picked for a query* and nudges it up
//! next time. That is the entire feature: no profiling, no embeddings, no
//! upload. The file lives next to the index cache and you can delete it at any
//! time with `localsearch --reset-usage`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use search_core::clock;

/// One remembered selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageEntry {
    /// The entity that was selected.
    pub entity_id: String,
    /// The query text that was active when it was selected.
    pub query: String,
    /// How many times it has been selected.
    pub selection_count: u32,
    /// When it was last selected, in Unix milliseconds.
    pub last_selected_ms: i64,
}

/// Bounded, local, explainable usage memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageIndex {
    entries: HashMap<String, UsageEntry>,
    /// Upper bound on remembered entities. Oldest entries are evicted first.
    max_entries: usize,
}

impl Default for UsageIndex {
    fn default() -> Self {
        Self::with_capacity(2_048)
    }
}

impl UsageIndex {
    /// Create an index bounded to `max_entries`.
    #[must_use]
    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            max_entries: max_entries.max(16),
        }
    }

    /// How many entities are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is remembered yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remember that `entity_id` was chosen for `query`.
    pub fn record(&mut self, query: &str, entity_id: &str) {
        if entity_id.is_empty() {
            return;
        }
        let now = clock::now_ms();
        let entry = self
            .entries
            .entry(entity_id.to_string())
            .or_insert_with(|| UsageEntry {
                entity_id: entity_id.to_string(),
                query: query.to_string(),
                selection_count: 0,
                last_selected_ms: now,
            });
        entry.selection_count = entry.selection_count.saturating_add(1);
        entry.last_selected_ms = now;
        entry.query = query.to_string();
        self.prune();
    }

    /// Selection count for an entity.
    #[must_use]
    pub fn count(&self, entity_id: &str) -> u32 {
        self.entries
            .get(entity_id)
            .map_or(0, |entry| entry.selection_count)
    }

    /// Normalised boost in `0.0..=1.0`.
    ///
    /// Frequency is logarithmic so a result picked 40 times does not bury
    /// everything else, and the whole term decays with a 30 day half-life so
    /// stale habits fade out.
    #[must_use]
    pub fn boost(&self, entity_id: &str) -> f32 {
        let Some(entry) = self.entries.get(entity_id) else {
            return 0.0;
        };
        let max_count = self
            .entries
            .values()
            .map(|candidate| candidate.selection_count)
            .max()
            .unwrap_or(1)
            .max(1);
        let frequency = ((1.0 + entry.selection_count as f64).ln() / (1.0 + max_count as f64).ln())
            .clamp(0.0, 1.0);
        let age_days = ((clock::now_ms() - entry.last_selected_ms).max(0) as f64) / 86_400_000.0;
        let decay = 0.5_f64.powf(age_days / 30.0);
        (frequency * decay).clamp(0.0, 1.0) as f32
    }

    /// The remembered entry for an entity, if any.
    #[must_use]
    pub fn get(&self, entity_id: &str) -> Option<&UsageEntry> {
        self.entries.get(entity_id)
    }

    /// All remembered entries, ordered by most recently selected.
    #[must_use]
    pub fn recent(&self, limit: usize) -> Vec<UsageEntry> {
        let mut entries: Vec<UsageEntry> = self.entries.values().cloned().collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_selected_ms));
        entries.truncate(limit);
        entries
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    fn prune(&mut self) {
        if self.entries.len() <= self.max_entries {
            return;
        }
        let mut by_age: Vec<(String, i64)> = self
            .entries
            .iter()
            .map(|(key, entry)| (key.clone(), entry.last_selected_ms))
            .collect();
        by_age.sort_by_key(|(_, last_selected)| *last_selected);
        let excess = self.entries.len() - self.max_entries;
        for (key, _) in by_age.into_iter().take(excess) {
            self.entries.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_increments_the_selection_count() {
        let mut usage = UsageIndex::default();
        usage.record("chrome", "app:c:\\chrome.exe");
        usage.record("chrome", "app:c:\\chrome.exe");
        assert_eq!(usage.count("app:c:\\chrome.exe"), 2);
        assert_eq!(usage.count("app:other"), 0);
    }

    #[test]
    fn unknown_entities_get_no_boost() {
        let usage = UsageIndex::default();
        assert_eq!(usage.boost("app:nothing"), 0.0);
    }

    #[test]
    fn frequently_selected_entities_are_boosted() {
        let mut usage = UsageIndex::default();
        usage.record("q", "app:frequent");
        usage.record("q", "app:frequent");
        usage.record("q", "app:frequent");
        usage.record("q", "app:rare");
        assert!(usage.boost("app:frequent") > usage.boost("app:rare"));
    }

    #[test]
    fn boost_is_bounded_to_zero_and_one() {
        let mut usage = UsageIndex::default();
        for _ in 0..500 {
            usage.record("q", "app:x");
        }
        let boost = usage.boost("app:x");
        assert!((0.0..=1.0).contains(&boost), "boost was {boost}");
    }

    #[test]
    fn capacity_is_enforced_by_evicting_the_oldest() {
        let mut usage = UsageIndex::with_capacity(16);
        for index in 0..40 {
            usage.record("q", &format!("app:{index}"));
        }
        assert!(usage.len() <= 16);
    }

    #[test]
    fn clearing_removes_everything() {
        let mut usage = UsageIndex::default();
        usage.record("q", "app:x");
        usage.clear();
        assert!(usage.is_empty());
    }
}
