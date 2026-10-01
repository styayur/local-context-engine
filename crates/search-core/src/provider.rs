//! The provider boundary.
//!
//! A provider is the only place that knows how to read one domain out of the
//! operating system. It returns normalised [`LocalEntity`] values and nothing
//! else — no ranking, no formatting, no presentation concerns.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::entity::{EntityType, LocalEntity};
use crate::error::Result;
use crate::query::SearchQuery;

/// How fresh a provider's data is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotScope {
    /// Read live from the OS on every search (processes, services, windows).
    Live,
    /// Served from a persisted index (files).
    Cached,
    /// A cached index with live elements layered on top.
    Hybrid,
}

impl SnapshotScope {
    /// Lower-case wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SnapshotScope::Live => "live",
            SnapshotScope::Cached => "cached",
            SnapshotScope::Hybrid => "hybrid",
        }
    }
}

/// Health and size information for one provider, surfaced by the UI's index
/// panel and by `localsearch --index-status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderStats {
    /// Provider name.
    pub name: String,
    /// How fresh the data is.
    pub scope: SnapshotScope,
    /// Entity types this provider serves.
    pub entity_types: Vec<EntityType>,
    /// Number of entities currently held, if known.
    pub entity_count: Option<usize>,
    /// Whether the provider can answer queries right now.
    pub ready: bool,
    /// Free-form detail (active backend, volume list, and so on).
    pub detail: BTreeMap<String, String>,
    /// Recoverable problems worth showing to the user.
    pub warnings: Vec<String>,
}

impl ProviderStats {
    /// Build minimal stats for a provider.
    #[must_use]
    pub fn new(name: impl Into<String>, scope: SnapshotScope, ready: bool) -> Self {
        Self {
            name: name.into(),
            scope,
            entity_types: Vec::new(),
            entity_count: None,
            ready,
            detail: BTreeMap::new(),
            warnings: Vec::new(),
        }
    }
}

/// A source of searchable entities.
pub trait EntityProvider: Send + Sync + std::fmt::Debug {
    /// Stable provider name, for example `files`.
    fn name(&self) -> &'static str;

    /// How fresh this provider's data is.
    fn scope(&self) -> SnapshotScope;

    /// Entity types this provider can produce.
    fn entity_types(&self) -> &'static [EntityType];

    /// Collect every entity that satisfies the query's structured predicates.
    ///
    /// Implementations should return an empty vector (not an error) when the
    /// query does not ask for any of their entity types.
    fn collect(&self, query: &SearchQuery) -> Result<Vec<LocalEntity>>;

    /// Current health/size information.
    fn stats(&self) -> ProviderStats {
        ProviderStats {
            name: self.name().to_string(),
            scope: self.scope(),
            entity_types: self.entity_types().to_vec(),
            entity_count: None,
            ready: true,
            detail: BTreeMap::new(),
            warnings: Vec::new(),
        }
    }

    /// Re-read whatever this provider caches. Default is a no-op, which is the
    /// right answer for live providers.
    fn refresh(&self) -> Result<()> {
        Ok(())
    }
}
