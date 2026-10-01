//! The window [`EntityProvider`].

use std::sync::Mutex;
use std::time::{Duration, Instant};

use search_core::{
    passes_filters, EntityProvider, EntityType, LceError, LocalEntity, ProviderStats, SearchQuery,
    SnapshotScope,
};

use crate::snapshot::list_windows;

/// Live top-level window provider.
#[derive(Debug)]
pub struct WindowProvider {
    cache: Mutex<Option<Cached>>,
    ttl: Duration,
}

impl Default for WindowProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct Cached {
    taken_at: Instant,
    entries: Vec<LocalEntity>,
}

impl WindowProvider {
    /// Snapshot lifetime used when a caller does not pick one.
    pub const DEFAULT_TTL: Duration = Duration::from_millis(250);

    /// Build a provider with the default TTL.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(None),
            ttl: Self::DEFAULT_TTL,
        }
    }

    /// Build a provider with an explicit snapshot lifetime.
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            cache: Mutex::new(None),
            ttl,
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

        let entities: Vec<LocalEntity> = list_windows()?
            .into_iter()
            .map(LocalEntity::Window)
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

impl EntityProvider for WindowProvider {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn scope(&self) -> SnapshotScope {
        SnapshotScope::Live
    }

    fn entity_types(&self) -> &'static [EntityType] {
        &[EntityType::Window]
    }

    fn collect(&self, query: &SearchQuery) -> Result<Vec<LocalEntity>, LceError> {
        if !query.accepts_type(EntityType::Window) {
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
        stats.detail.insert("api".into(), "enumwindows".into());
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

    #[test]
    fn window_provider_declares_the_window_entity_type() {
        let provider = WindowProvider::new();
        assert_eq!(provider.entity_types(), &[EntityType::Window]);
        assert_eq!(provider.scope(), SnapshotScope::Live);
    }

    #[test]
    fn other_entity_types_are_skipped() {
        let provider = WindowProvider::new();
        let query = SearchQuery::plain("x").with_types([EntityType::File]);
        assert!(provider.collect(&query).unwrap().is_empty());
    }

    #[test]
    fn window_snapshot_entries_are_well_formed() {
        let provider = WindowProvider::new();
        let entities = provider.collect(&SearchQuery::plain("")).unwrap();
        for entity in entities {
            match entity {
                LocalEntity::Window(entry) => assert!(!entry.title.trim().is_empty()),
                other => panic!("expected a window, got {other:?}"),
            }
        }
    }
}
