//! The service [`EntityProvider`].

use std::sync::Mutex;
use std::time::{Duration, Instant};

use search_core::{
    passes_filters, EntityProvider, EntityType, LceError, LocalEntity, ProviderStats, SearchQuery,
    SnapshotScope,
};

use crate::snapshot::{list_services_with, ServiceQueryHint};

/// Read-only service provider with a short-lived snapshot cache.
#[derive(Debug)]
pub struct ServiceProvider {
    cache: Mutex<Option<Cached>>,
    ttl: Duration,
}

impl Default for ServiceProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct Cached {
    taken_at: Instant,
    entries: Vec<LocalEntity>,
}

impl ServiceProvider {
    /// Snapshot lifetime used when a caller does not pick one.
    ///
    /// Service state changes rarely, so this caches for longer than the
    /// process and window providers.
    pub const DEFAULT_TTL: Duration = Duration::from_secs(5);

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

    fn snapshot(&self, hint: &ServiceQueryHint) -> Result<Vec<LocalEntity>, LceError> {
        if let Ok(guard) = self.cache.lock() {
            if let Some(cached) = guard.as_ref() {
                if cached.taken_at.elapsed() < self.ttl {
                    return Ok(cached.entries.clone());
                }
            }
        }

        let entities: Vec<LocalEntity> = list_services_with(hint)?
            .into_iter()
            .map(LocalEntity::Service)
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

impl EntityProvider for ServiceProvider {
    fn name(&self) -> &'static str {
        "services"
    }

    fn scope(&self) -> SnapshotScope {
        SnapshotScope::Live
    }

    fn entity_types(&self) -> &'static [EntityType] {
        &[EntityType::Service]
    }

    fn collect(&self, query: &SearchQuery) -> Result<Vec<LocalEntity>, LceError> {
        if !query.accepts_type(EntityType::Service) {
            return Ok(Vec::new());
        }
        Ok(self
            .snapshot(&hint_for(query))?
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
            }
        }
        stats
            .detail
            .insert("api".into(), "EnumServicesStatusEx".into());
        stats.detail.insert("mode".into(), "read-only".into());
        stats
    }

    fn refresh(&self) -> Result<(), LceError> {
        self.invalidate();
        Ok(())
    }
}

/// Decide how much per-service work this query actually needs.
///
/// `state:` and `path:` filters are answered from `QueryServiceConfig`, so any
/// query carrying one of those has to resolve every service. A plain text query
/// does not: only services whose name matches can ever reach the result list.
fn hint_for(query: &SearchQuery) -> ServiceQueryHint {
    let needs_full_config = query.filters.iter().any(|filter| {
        matches!(
            filter,
            search_core::Filter::State(_) | search_core::Filter::Path(_)
        )
    });
    let text_tokens = if needs_full_config {
        Vec::new()
    } else {
        query
            .text
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_lowercase)
            .collect()
    };
    ServiceQueryHint {
        text_tokens,
        needs_full_config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use search_core::Filter;

    #[test]
    fn service_provider_declares_the_service_entity_type() {
        let provider = ServiceProvider::new();
        assert_eq!(provider.entity_types(), &[EntityType::Service]);
        assert_eq!(provider.scope(), SnapshotScope::Live);
    }

    #[test]
    fn other_entity_types_are_skipped() {
        let provider = ServiceProvider::new();
        let query = SearchQuery::plain("x").with_types([EntityType::File]);
        assert!(provider.collect(&query).unwrap().is_empty());
    }

    #[test]
    fn running_state_filter_returns_only_running_services() {
        let provider = ServiceProvider::new();
        let query = SearchQuery::plain("")
            .with_types([EntityType::Service])
            .with_filter(Filter::State("running".into()));
        let entities = provider.collect(&query).unwrap();
        for entity in entities {
            match entity {
                LocalEntity::Service(entry) => {
                    assert_eq!(entry.state, search_core::ServiceState::Running);
                }
                other => panic!("expected a service, got {other:?}"),
            }
        }
    }

    #[test]
    fn text_queries_avoid_the_expensive_config_lookup() {
        let hint = hint_for(&SearchQuery::plain("nvidia"));
        assert!(!hint.needs_full_config);
        assert_eq!(hint.text_tokens, vec!["nvidia".to_string()]);
        assert!(hint.wants_config("nvlddmkm", "NVIDIA Display Driver Service"));
        assert!(!hint.wants_config("wuauserv", "Windows Update"));
    }

    #[test]
    fn state_filters_force_a_full_config_pass() {
        let query = SearchQuery::plain("").with_filter(Filter::State("running".into()));
        assert!(hint_for(&query).needs_full_config);
    }

    #[test]
    fn a_query_without_text_resolves_everything() {
        // An empty token list means "no text to narrow by", which the hint
        // treats as "resolve everything" rather than "resolve nothing".
        let hint = hint_for(&SearchQuery::plain(""));
        assert!(hint.wants_config("wuauserv", "Windows Update"));
        assert!(hint.wants_config("anything", "Anything"));
    }

    #[test]
    fn the_provider_is_read_only() {
        let stats = ServiceProvider::new().stats();
        assert_eq!(
            stats.detail.get("mode").map(String::as_str),
            Some("read-only")
        );
    }
}
