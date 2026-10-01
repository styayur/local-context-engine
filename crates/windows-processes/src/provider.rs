//! The process [`EntityProvider`].
//!
//! The snapshot is live, but memoised for a very short window so that typing
//! in the search box does not re-enumerate the process table on every
//! keystroke. The default TTL is deliberately tiny: see
//! [`ProcessProvider::DEFAULT_TTL`].

use std::sync::Mutex;
use std::time::{Duration, Instant};

use search_core::{
    passes_filters, EntityProvider, EntityType, LceError, LocalEntity, ProviderStats, SearchQuery,
    SnapshotScope,
};

use crate::snapshot::{list_processes_with, ProcessOptions};

/// Live process provider with a short-lived snapshot cache.
#[derive(Debug)]
pub struct ProcessProvider {
    cache: Mutex<Option<Cached>>,
    ttl: Duration,
    options: ProcessOptions,
}

impl Default for ProcessProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct Cached {
    taken_at: Instant,
    entries: Vec<LocalEntity>,
}

impl ProcessProvider {
    /// Snapshot lifetime. Long enough to coalesce a burst of keystrokes, short
    /// enough that the list is always "now" to a human.
    pub const DEFAULT_TTL: Duration = Duration::from_millis(250);

    /// Build a provider with the default TTL.
    #[must_use]
    pub fn new() -> Self {
        Self::with_options(ProcessOptions::default())
    }

    /// Build a provider that resolves exactly the requested details.
    #[must_use]
    pub fn with_options(options: ProcessOptions) -> Self {
        Self {
            cache: Mutex::new(None),
            ttl: Self::DEFAULT_TTL,
            options,
        }
    }

    /// Build a provider with an explicit snapshot lifetime.
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            cache: Mutex::new(None),
            ttl,
            options: ProcessOptions::default(),
        }
    }

    /// Drop the memoised snapshot.
    pub fn invalidate(&self) {
        if let Ok(mut guard) = self.cache.lock() {
            *guard = None;
        }
    }

    fn snapshot(&self) -> Result<Vec<LocalEntity>, LceError> {
        if let Ok(guard) = self.cache.lock() {
            if let Some(cached) = guard.as_ref() {
                if cached.taken_at.elapsed() < self.ttl {
                    return Ok(cached.entries.clone());
                }
            }
        }

        let entities: Vec<LocalEntity> = list_processes_with(self.options)?
            .into_iter()
            .map(LocalEntity::Process)
            .collect();

        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some(Cached {
                taken_at: Instant::now(),
                entries: entities.clone(),
            });
        }
        Ok(entities)
    }
}

impl EntityProvider for ProcessProvider {
    fn name(&self) -> &'static str {
        "processes"
    }

    fn scope(&self) -> SnapshotScope {
        SnapshotScope::Live
    }

    fn entity_types(&self) -> &'static [EntityType] {
        &[EntityType::Process]
    }

    fn collect(&self, query: &SearchQuery) -> Result<Vec<LocalEntity>, LceError> {
        if !query.accepts_type(EntityType::Process) {
            return Ok(Vec::new());
        }
        Ok(self
            .snapshot()?
            .into_iter()
            .filter(|entity| passes_filters(entity, query))
            .collect())
    }

    fn stats(&self) -> ProviderStats {
        let mut stats = ProviderStats::new(self.name(), self.scope(), true);
        stats.entity_types = self.entity_types().to_vec();
        if let Ok(guard) = self.cache.lock() {
            if let Some(cached) = guard.as_ref() {
                stats.entity_count = Some(cached.entries.len());
                stats.detail.insert(
                    "cache_age_ms".into(),
                    cached.taken_at.elapsed().as_millis().to_string(),
                );
            }
        }
        stats
            .detail
            .insert("snapshot_ttl_ms".into(), self.ttl.as_millis().to_string());
        stats.detail.insert("api".into(), "toolhelp32+psapi".into());
        stats
            .detail
            .insert("account_lookup".into(), self.options.username.to_string());
        stats
    }

    fn refresh(&self) -> Result<(), LceError> {
        self.invalidate();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use search_core::Filter;

    #[test]
    fn provider_declares_the_process_entity_type() {
        let provider = ProcessProvider::new();
        assert_eq!(provider.entity_types(), &[EntityType::Process]);
        assert_eq!(provider.scope(), SnapshotScope::Live);
    }

    #[test]
    fn queries_for_other_entity_types_are_skipped_without_touching_the_os() {
        let provider = ProcessProvider::new();
        let query = SearchQuery::plain("x").with_types([EntityType::Service]);
        assert!(provider.collect(&query).unwrap().is_empty());
    }

    #[test]
    fn the_live_snapshot_contains_at_least_the_current_process() {
        let provider = ProcessProvider::new();
        let query = SearchQuery::plain("");
        let entities = provider.collect(&query).unwrap();
        assert!(
            !entities.is_empty(),
            "a running system always has at least one process"
        );
        let self_pid = std::process::id();
        assert!(
            entities.iter().any(|entity| matches!(
                entity,
                LocalEntity::Process(entry) if entry.pid == self_pid
            )),
            "the test process itself must appear in the snapshot"
        );
    }

    #[test]
    fn pid_filters_restrict_the_snapshot() {
        let provider = ProcessProvider::new();
        let self_pid = std::process::id();
        let query = SearchQuery::plain("").with_filter(Filter::Pid(self_pid));
        let entities = provider.collect(&query).unwrap();
        assert_eq!(entities.len(), 1);
        match &entities[0] {
            LocalEntity::Process(entry) => assert_eq!(entry.pid, self_pid),
            other => panic!("expected a process, got {other:?}"),
        }
    }

    #[test]
    fn stats_report_the_cached_entity_count() {
        let provider = ProcessProvider::new();
        let _ = provider.collect(&SearchQuery::plain("")).unwrap();
        let stats = provider.stats();
        assert!(stats.entity_count.unwrap_or(0) > 0);
    }
}
