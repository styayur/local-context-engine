//! The application [`EntityProvider`].

use std::sync::Mutex;
use std::time::{Duration, Instant};

use search_core::{
    passes_filters, AppSource, EntityProvider, EntityType, LceError, LocalEntity, ProviderStats,
    SearchQuery, SnapshotScope,
};

use crate::discovery::AppCatalogue;

/// Installed-application provider.
///
/// The catalogue is built lazily on first use and then held until it expires or
/// [`AppProvider::refresh`] is called: installed applications change on the
/// order of days, not milliseconds.
#[derive(Debug)]
pub struct AppProvider {
    cache: Mutex<Option<Cached>>,
    ttl: Duration,
}

impl Default for AppProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct Cached {
    taken_at: Instant,
    entries: Vec<LocalEntity>,
    counts: Vec<(AppSource, usize)>,
}

impl AppProvider {
    /// How long a discovered catalogue is trusted.
    pub const DEFAULT_TTL: Duration = Duration::from_secs(300);

    /// Build a provider with the default TTL.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(None),
            ttl: Self::DEFAULT_TTL,
        }
    }

    /// Build a provider with an explicit catalogue lifetime.
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            cache: Mutex::new(None),
            ttl,
        }
    }

    /// Rebuild the catalogue now.
    pub fn invalidate(&self) {
        if let Ok(mut guard) = self.cache.lock() {
            *guard = None;
        }
    }

    fn snapshot(&self) -> Vec<LocalEntity> {
        if let Ok(guard) = self.cache.lock() {
            if let Some(cached) = guard.as_ref() {
                if cached.taken_at.elapsed() < self.ttl {
                    return cached.entries.clone();
                }
            }
        }

        let catalogue = AppCatalogue::build();
        let counts: Vec<(AppSource, usize)> = {
            let mut pairs: Vec<(AppSource, usize)> =
                catalogue.source_counts().into_iter().collect();
            pairs.sort_by_key(|(source, _)| source.as_str());
            pairs
        };
        let entries: Vec<LocalEntity> = catalogue
            .entries()
            .iter()
            .cloned()
            .map(LocalEntity::Application)
            .collect();

        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some(Cached {
                taken_at: Instant::now(),
                entries: entries.clone(),
                counts,
            });
        }
        entries
    }
}

impl EntityProvider for AppProvider {
    fn name(&self) -> &'static str {
        "apps"
    }

    fn scope(&self) -> SnapshotScope {
        SnapshotScope::Cached
    }

    fn entity_types(&self) -> &'static [EntityType] {
        &[EntityType::Application]
    }

    fn collect(&self, query: &SearchQuery) -> Result<Vec<LocalEntity>, LceError> {
        if !query.accepts_type(EntityType::Application) {
            return Ok(Vec::new());
        }
        Ok(self
            .snapshot()
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
                for (source, count) in &cached.counts {
                    stats
                        .detail
                        .insert(format!("source.{}", source.as_str()), count.to_string());
                }
            }
        }
        stats.detail.insert(
            "sources".into(),
            "app-paths,start-menu,path,windows-apps".into(),
        );
        stats
    }

    fn refresh(&self) -> Result<(), LceError> {
        self.invalidate();
        let _ = self.snapshot();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_provider_declares_the_application_entity_type() {
        let provider = AppProvider::new();
        assert_eq!(provider.entity_types(), &[EntityType::Application]);
        assert_eq!(provider.scope(), SnapshotScope::Cached);
    }

    #[test]
    fn other_entity_types_are_skipped() {
        let provider = AppProvider::new();
        let query = SearchQuery::plain("x").with_types([EntityType::Process]);
        assert!(provider.collect(&query).unwrap().is_empty());
    }

    #[test]
    fn the_catalogue_contains_applications_on_a_real_machine() {
        let provider = AppProvider::new();
        let entities = provider.collect(&SearchQuery::plain("")).unwrap();
        assert!(!entities.is_empty(), "expected at least one application");
        for entity in &entities {
            assert_eq!(entity.entity_type(), EntityType::Application);
        }
    }

    #[test]
    fn refresh_rebuilds_the_catalogue() {
        let provider = AppProvider::new();
        let first = provider.collect(&SearchQuery::plain("")).unwrap().len();
        provider.refresh().unwrap();
        let second = provider.collect(&SearchQuery::plain("")).unwrap().len();
        assert_eq!(first, second);
    }

    #[test]
    fn stats_report_per_source_counts() {
        let provider = AppProvider::new();
        let _ = provider.collect(&SearchQuery::plain("")).unwrap();
        let stats = provider.stats();
        assert!(stats.entity_count.unwrap_or(0) > 0);
        assert!(stats.detail.contains_key("sources"));
    }
}
