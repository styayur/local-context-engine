//! The structured query model.
//!
//! A [`SearchQuery`] is the *only* thing the search engine understands. Human
//! input — the DSL, natural language, CLI flags — is compiled into this struct
//! before it ever reaches the engine, which keeps retrieval deterministic and
//! makes the whole pipeline testable without any AI in the loop.

use std::fmt::Write as _;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::entity::EntityType;
use crate::result::DEFAULT_RESULT_LIMIT;

/// The full description of what the caller wants back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    /// Free text matched against names and paths.
    pub text: Option<String>,
    /// Restricted entity types. Empty means "all types".
    pub entity_types: Vec<EntityType>,
    /// Structured predicates applied after the text match.
    pub filters: Vec<Filter>,
    /// Explicit sort order. `None` means "by relevance".
    pub sort: Option<Sort>,
    /// Maximum number of results.
    pub limit: usize,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            text: None,
            entity_types: Vec::new(),
            filters: Vec::new(),
            sort: None,
            limit: DEFAULT_RESULT_LIMIT,
        }
    }
}

impl SearchQuery {
    /// A query that matches nothing but is valid: handy as a builder start.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Plain keyword search across every entity type.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        let trimmed = text.trim();
        Self {
            text: (!trimmed.is_empty()).then(|| trimmed.to_string()),
            ..Self::default()
        }
    }

    /// Restrict the query to the given entity types.
    #[must_use]
    pub fn with_types(mut self, types: impl IntoIterator<Item = EntityType>) -> Self {
        self.entity_types = types.into_iter().collect();
        self
    }

    /// Add one predicate.
    #[must_use]
    pub fn with_filter(mut self, filter: Filter) -> Self {
        self.filters.push(filter);
        self
    }

    /// Set an explicit sort order.
    #[must_use]
    pub fn with_sort(mut self, sort: Sort) -> Self {
        self.sort = Some(sort);
        self
    }

    /// Clamp the result limit, never allowing zero or an unbounded value.
    #[must_use]
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit.clamp(1, crate::result::MAX_RESULT_LIMIT);
        self
    }

    /// Whether this query is empty (no text and no predicates).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.as_deref().is_none_or(str::is_empty) && self.filters.is_empty()
    }

    /// Whether the given entity type passes the type restriction.
    #[must_use]
    pub fn accepts_type(&self, entity_type: EntityType) -> bool {
        self.entity_types.is_empty() || self.entity_types.contains(&entity_type)
    }

    /// Render back to canonical DSL. Used by `--explain`, by the MCP tool
    /// output and by round-trip tests.
    #[must_use]
    pub fn to_dsl(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for entity_type in &self.entity_types {
            parts.push(format!("type:{entity_type}"));
        }
        for filter in &self.filters {
            parts.push(filter.to_dsl());
        }
        if let Some(sort) = &self.sort {
            parts.push(format!("sort:{}", sort.to_dsl()));
        }
        if let Some(text) = self.text.as_deref() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }
        if self.limit != DEFAULT_RESULT_LIMIT {
            parts.push(format!("limit:{}", self.limit));
        }
        let mut out = String::new();
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            let _ = write!(out, "{part}");
        }
        out
    }
}

/// A structured predicate applied after text matching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "filter", content = "value", rename_all = "kebab-case")]
pub enum Filter {
    /// `ext:rs` — extension match, compared without the leading dot.
    Extension(String),
    /// `path:projects` — substring match against the full path.
    Path(String),
    /// `drive:C` — restrict to a volume.
    Drive(char),
    /// `name:node` — substring match against the entity name.
    Name(String),
    /// `modified:<24h` / `modified:>2024-01-01`.
    Modified(TimeBound),
    /// `created:<7d`.
    Created(TimeBound),
    /// `size:>10mb` — only meaningful for files.
    Size(SizeFilter),
    /// `state:running` — service or process state.
    State(String),
    /// `pid:1234`.
    Pid(u32),
    /// `user:alice` — owning account.
    User(String),
    /// `visible:true` — window visibility.
    Visible(bool),
}

impl Filter {
    /// Canonical DSL spelling.
    #[must_use]
    pub fn to_dsl(&self) -> String {
        match self {
            Filter::Extension(value) => format!("ext:{value}"),
            Filter::Path(value) => quote_if_needed("path", value),
            Filter::Drive(letter) => format!("drive:{letter}"),
            Filter::Name(value) => quote_if_needed("name", value),
            Filter::Modified(bound) => format!("modified:{}", bound.to_dsl()),
            Filter::Created(bound) => format!("created:{}", bound.to_dsl()),
            Filter::Size(size) => format!("size:{}", size.to_dsl()),
            Filter::State(value) => format!("state:{value}"),
            Filter::Pid(pid) => format!("pid:{pid}"),
            Filter::User(value) => quote_if_needed("user", value),
            Filter::Visible(visible) => format!("visible:{visible}"),
        }
    }
}

fn quote_if_needed(key: &str, value: &str) -> String {
    if value.contains(' ') {
        format!("{key}:\"{value}\"")
    } else {
        format!("{key}:{value}")
    }
}

/// A relative or absolute time bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum TimeBound {
    /// Newer than `now - duration` (DSL: `modified:<24h`).
    Within(Duration),
    /// Older than `now - duration` (DSL: `modified:>24h`).
    OlderThan(Duration),
    /// Newer than an absolute instant, Unix milliseconds (DSL: `modified:>=2024-01-01`).
    After(i64),
    /// Older than an absolute instant, Unix milliseconds (DSL: `modified:<=2024-01-01`).
    Before(i64),
}

impl TimeBound {
    /// Canonical DSL spelling.
    #[must_use]
    pub fn to_dsl(&self) -> String {
        match self {
            TimeBound::Within(duration) => format!("<{}", crate::clock::format_duration(*duration)),
            TimeBound::OlderThan(duration) => {
                format!(">{}", crate::clock::format_duration(*duration))
            }
            TimeBound::After(ms) => format!(">={}", crate::clock::format_unix_ms(*ms)),
            TimeBound::Before(ms) => format!("<={}", crate::clock::format_unix_ms(*ms)),
        }
    }
}

/// Comparison operator for `size:` filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SizeOp {
    GreaterThan,
    GreaterOrEqual,
    LessThan,
    LessOrEqual,
    Equal,
}

impl SizeOp {
    /// Canonical DSL spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SizeOp::GreaterThan => ">",
            SizeOp::GreaterOrEqual => ">=",
            SizeOp::LessThan => "<",
            SizeOp::LessOrEqual => "<=",
            SizeOp::Equal => "=",
        }
    }

    /// Whether `value` satisfies the comparison against `threshold`.
    #[must_use]
    pub const fn matches(self, value: u64, threshold: u64) -> bool {
        match self {
            SizeOp::GreaterThan => value > threshold,
            SizeOp::GreaterOrEqual => value >= threshold,
            SizeOp::LessThan => value < threshold,
            SizeOp::LessOrEqual => value <= threshold,
            SizeOp::Equal => value == threshold,
        }
    }
}

/// A parsed `size:` predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SizeFilter {
    /// Comparison operator.
    pub op: SizeOp,
    /// Threshold in bytes.
    pub bytes: u64,
}

impl SizeFilter {
    /// Canonical DSL spelling.
    #[must_use]
    pub fn to_dsl(&self) -> String {
        format!(
            "{}{}",
            self.op.as_str(),
            crate::clock::format_size(self.bytes)
        )
    }
}

/// Sort key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortKey {
    Relevance,
    Name,
    Path,
    Modified,
    Created,
    Size,
    Pid,
    Memory,
}

impl SortKey {
    /// Canonical DSL spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SortKey::Relevance => "relevance",
            SortKey::Name => "name",
            SortKey::Path => "path",
            SortKey::Modified => "modified",
            SortKey::Created => "created",
            SortKey::Size => "size",
            SortKey::Pid => "pid",
            SortKey::Memory => "memory",
        }
    }

    /// Parse the `key` half of `sort:<key>-<direction>`.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.to_ascii_lowercase().as_str() {
            "relevance" | "score" => SortKey::Relevance,
            "name" | "filename" => SortKey::Name,
            "path" => SortKey::Path,
            "modified" | "mtime" | "date" => SortKey::Modified,
            "created" | "ctime" => SortKey::Created,
            "size" => SortKey::Size,
            "pid" => SortKey::Pid,
            "memory" | "mem" => SortKey::Memory,
            _ => return None,
        })
    }
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    /// Canonical DSL spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SortDirection::Asc => "asc",
            SortDirection::Desc => "desc",
        }
    }

    /// Parse `asc`/`desc` (also accepts `ascending`/`descending`).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.to_ascii_lowercase().as_str() {
            "asc" | "ascending" | "up" => SortDirection::Asc,
            "desc" | "descending" | "down" => SortDirection::Desc,
            _ => return None,
        })
    }
}

/// An explicit sort order (`sort:size-desc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sort {
    /// Field to order by.
    pub key: SortKey,
    /// Direction.
    pub direction: SortDirection,
}

impl Sort {
    /// Build a sort order.
    #[must_use]
    pub const fn new(key: SortKey, direction: SortDirection) -> Self {
        Self { key, direction }
    }

    /// Newest first (`sort:modified-desc`).
    #[must_use]
    pub const fn modified_desc() -> Self {
        Self::new(SortKey::Modified, SortDirection::Desc)
    }

    /// Canonical DSL spelling, for example `size-desc`.
    #[must_use]
    pub fn to_dsl(&self) -> String {
        format!("{}-{}", self.key.as_str(), self.direction.as_str())
    }

    /// Parse the value half of a `sort:` token.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        // Accept `size-desc`, `size:desc`, `size` and `size_desc`.
        let normalized = trimmed.replace([':', '_'], "-");
        match normalized.rsplit_once('-') {
            Some((key, direction)) => {
                let key = SortKey::parse(key)?;
                let direction = SortDirection::parse(direction)?;
                Some(Self { key, direction })
            }
            None => Some(Self {
                key: SortKey::parse(&normalized)?,
                direction: SortDirection::Desc,
            }),
        }
    }
}
