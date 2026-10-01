//! Result and response types shared by the CLI, the MCP server and the UI.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::entity::{EntityType, LocalEntity};

/// Result count used when the caller does not ask for a specific limit.
pub const DEFAULT_RESULT_LIMIT: usize = 50;
/// Hard ceiling so a runaway query can never allocate an unbounded list.
pub const MAX_RESULT_LIMIT: usize = 2_000;

/// A half-open range of **character** offsets inside a result field, used by
/// the UI to highlight what matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchRange {
    /// First matching character index.
    pub start: usize,
    /// One past the last matching character index.
    pub end: usize,
}

/// Which field the highlighted ranges refer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum MatchField {
    /// Highlight ranges apply to [`SearchResult::name`].
    Name,
    /// Highlight ranges apply to [`SearchResult::path`].
    Path,
    /// Nothing matched on a highlighted field.
    #[default]
    None,
}

/// One search hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    /// Stable identity, see [`LocalEntity::id`].
    pub id: String,
    /// Domain of the hit.
    pub entity_type: EntityType,
    /// Primary matching string (file name, process name, window title, ...).
    pub name: String,
    /// Name to render; falls back to `name`.
    pub display_name: String,
    /// Filesystem location, when the entity has one.
    pub path: Option<String>,
    /// Secondary line for the UI.
    pub subtitle: String,
    /// Relevance score produced by the ranking crate.
    pub score: f32,
    /// Flat key/value metadata for consumers that do not want the typed entity.
    pub metadata: BTreeMap<String, String>,
    /// Character ranges inside `name` (or `path`) to highlight.
    pub match_ranges: Vec<MatchRange>,
    /// Which field `match_ranges` refers to.
    pub matched_field: MatchField,
    /// The typed entity, so the UI can build context actions without guessing.
    pub entity: LocalEntity,
}

impl SearchResult {
    /// Build a result from an entity plus a score.
    #[must_use]
    pub fn from_entity(entity: LocalEntity, score: f32) -> Self {
        let id = entity.id();
        let entity_type = entity.entity_type();
        let name = entity.name().to_string();
        let display_name = entity.display_name();
        let path = entity.path().map(str::to_string);
        let subtitle = entity.subtitle();
        let metadata = entity.metadata();
        Self {
            id,
            entity_type,
            name,
            display_name,
            path,
            subtitle,
            score,
            metadata,
            match_ranges: Vec::new(),
            matched_field: MatchField::None,
            entity,
        }
    }

    /// Attach highlight ranges to the name.
    #[must_use]
    pub fn with_name_highlights(mut self, ranges: Vec<MatchRange>) -> Self {
        if !ranges.is_empty() {
            self.match_ranges = ranges;
            self.matched_field = MatchField::Name;
        }
        self
    }

    /// Attach highlight ranges to the path.
    #[must_use]
    pub fn with_path_highlights(mut self, ranges: Vec<MatchRange>) -> Self {
        if !ranges.is_empty() {
            self.match_ranges = ranges;
            self.matched_field = MatchField::Path;
        }
        self
    }
}

/// Per-provider timing, surfaced in `--explain` and the UI footer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderTiming {
    /// Provider name, for example `files`.
    pub provider: String,
    /// Number of candidate entities the provider considered.
    pub candidates: usize,
    /// Wall-clock time the provider took, in milliseconds.
    pub elapsed_ms: f64,
}

/// The complete answer to one search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResponse {
    /// The raw user input.
    pub query: String,
    /// The compiled query rendered back as DSL. Empty when the input was DSL.
    pub compiled: String,
    /// Total wall-clock time in milliseconds.
    pub elapsed_ms: f64,
    /// Number of results returned.
    pub total: usize,
    /// Whether more results existed than `limit` allowed.
    pub truncated: bool,
    /// Ranked results.
    pub results: Vec<SearchResult>,
    /// Per-provider timings.
    pub timings: Vec<ProviderTiming>,
    /// Non-fatal problems (a volume that needs elevation, and so on).
    pub warnings: Vec<String>,
}

impl SearchResponse {
    /// An empty response for a given query string.
    #[must_use]
    pub fn empty(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            compiled: String::new(),
            elapsed_ms: 0.0,
            total: 0,
            truncated: false,
            results: Vec::new(),
            timings: Vec::new(),
            warnings: Vec::new(),
        }
    }
}
