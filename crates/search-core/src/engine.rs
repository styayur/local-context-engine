//! The deterministic search engine.
//!
//! The engine owns exactly two things: fan-out across providers, and turning
//! scored candidates into a sorted, limited [`SearchResponse`]. It contains no
//! domain knowledge and no text matching — both live behind the provider and
//! ranker boundaries so they can be tested and replaced independently.

use std::cmp::Ordering;
use std::sync::Arc;
use std::time::Instant;

use crate::entity::{EntityType, LocalEntity};
use crate::error::LceError;
use crate::matching::passes_filters;
use crate::provider::{EntityProvider, ProviderStats};
use crate::query::{SearchQuery, Sort, SortDirection, SortKey};
use crate::result::{
    MatchField, MatchRange, ProviderTiming, SearchResponse, SearchResult, DEFAULT_RESULT_LIMIT,
};
use crate::{clock, text};

/// The set of providers an engine fans out to.
pub type ProviderSet = Vec<Arc<dyn EntityProvider>>;

/// A scorer decides whether an entity answers the query's *text* part and how
/// well it does so.
///
/// Returning `None` means "this entity does not match the text" and drops it
/// from the result set. This is the seam where a different ranking strategy —
/// or a future personalised model — can be substituted without touching the
/// engine.
pub trait Ranker: Send + Sync + std::fmt::Debug {
    /// Score one candidate against the query.
    fn score(&self, query: &SearchQuery, entity: &LocalEntity) -> Option<RankedMatch>;
}

/// The outcome of scoring a single candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedMatch {
    /// Final relevance score.
    pub score: f32,
    /// Which field the highlight ranges refer to.
    pub field: MatchField,
    /// Character ranges to highlight.
    pub ranges: Vec<MatchRange>,
}

impl RankedMatch {
    /// A match with no highlighting.
    #[must_use]
    pub fn bare(score: f32) -> Self {
        Self {
            score,
            field: MatchField::None,
            ranges: Vec::new(),
        }
    }
}

/// Fuse providers and a ranker into one queryable engine.
#[derive(Debug)]
pub struct SearchEngine {
    providers: ProviderSet,
    ranker: Arc<dyn Ranker>,
}

impl SearchEngine {
    /// Build an engine.
    #[must_use]
    pub fn new(providers: ProviderSet, ranker: Arc<dyn Ranker>) -> Self {
        Self { providers, ranker }
    }

    /// The providers this engine was built with.
    #[must_use]
    pub fn providers(&self) -> &[Arc<dyn EntityProvider>] {
        &self.providers
    }

    /// Collect health information for every provider.
    #[must_use]
    pub fn provider_stats(&self) -> Vec<ProviderStats> {
        self.providers.iter().map(|p| p.stats()).collect()
    }

    /// Refresh every provider that caches data.
    ///
    /// Recoverable failures are returned alongside the number of providers
    /// that refreshed successfully; a single unreadable volume must never take
    /// the whole engine down.
    pub fn refresh_all(&self) -> (usize, Vec<LceError>) {
        let mut refreshed = 0;
        let mut errors = Vec::new();
        for provider in &self.providers {
            match provider.refresh() {
                Ok(()) => refreshed += 1,
                Err(error) => {
                    tracing::warn!(
                        provider = provider.name(),
                        code = error.code(),
                        error = %error,
                        "provider refresh failed"
                    );
                    errors.push(error);
                }
            }
        }
        (refreshed, errors)
    }

    /// Run one search.
    ///
    /// `raw` is the untouched user input and is echoed back in the response so
    /// the UI can show what was actually typed.
    #[must_use]
    pub fn search(&self, raw: &str, query: &SearchQuery) -> SearchResponse {
        let started = Instant::now();
        let limit = query.limit.clamp(1, crate::result::MAX_RESULT_LIMIT);
        let mut warnings = Vec::new();
        let mut timings = Vec::new();
        let mut scored: Vec<SearchResult> = Vec::new();
        // Collect a little more than requested so the sort has something to
        // work with, but never unbounded.
        let candidate_cap = limit.saturating_mul(8).max(64);

        for provider in &self.providers {
            if !provider
                .entity_types()
                .iter()
                .any(|entity_type| query.accepts_type(*entity_type))
            {
                continue;
            }

            let provider_started = Instant::now();
            let entities = match provider.collect(query) {
                Ok(entities) => entities,
                Err(error) => {
                    tracing::warn!(
                        provider = provider.name(),
                        code = error.code(),
                        error = %error,
                        "provider collection failed"
                    );
                    warnings.push(format!(
                        "{}: {} ({})",
                        provider.name(),
                        error.hint(),
                        error.code()
                    ));
                    timings.push(ProviderTiming {
                        provider: provider.name().to_string(),
                        candidates: 0,
                        elapsed_ms: elapsed_ms(provider_started),
                    });
                    continue;
                }
            };

            let candidates = entities.len();
            for entity in entities {
                if !passes_filters(&entity, query) {
                    continue;
                }
                if let Some(ranked) = self.ranker.score(query, &entity) {
                    scored.push(apply_rank(entity, ranked));
                }
            }
            timings.push(ProviderTiming {
                provider: provider.name().to_string(),
                candidates,
                elapsed_ms: elapsed_ms(provider_started),
            });

            if scored.len() > candidate_cap.saturating_mul(4) {
                scored.sort_by(relevance_desc);
                scored.truncate(candidate_cap);
            }
        }

        match query.sort {
            Some(sort) => sort_results(&mut scored, sort),
            None => scored.sort_by(relevance_desc),
        }

        if scored.is_empty() && query.text.as_deref().is_none_or(str::is_empty) {
            // A pure filter query (`type:process state:running`) is legitimate,
            // so an empty result set is only odd when text was supplied.
            warnings.clear();
        }

        let truncated = scored.len() > limit;
        scored.truncate(limit);
        for result in &mut scored {
            if result.match_ranges.is_empty() {
                if let Some(text) = query.text.as_deref() {
                    let ranges = text::match_ranges(text, &result.name);
                    if !ranges.is_empty() {
                        result.match_ranges = ranges;
                        result.matched_field = MatchField::Name;
                    }
                }
            }
        }

        SearchResponse {
            query: raw.to_string(),
            compiled: query.to_dsl(),
            elapsed_ms: elapsed_ms(started),
            total: scored.len(),
            truncated,
            results: scored,
            timings,
            warnings,
        }
    }

    /// Score a single entity without touching providers. Handy for tests and
    /// for `--explain`.
    #[must_use]
    pub fn score_entity(&self, query: &SearchQuery, entity: &LocalEntity) -> Option<RankedMatch> {
        self.ranker.score(query, entity)
    }
}

impl Default for SearchEngine {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            ranker: Arc::new(NoopRanker),
        }
    }
}

fn apply_rank(entity: LocalEntity, ranked: RankedMatch) -> SearchResult {
    let result = SearchResult::from_entity(entity, ranked.score);
    match ranked.field {
        MatchField::Name => result.with_name_highlights(ranked.ranges),
        MatchField::Path => result.with_path_highlights(ranked.ranges),
        MatchField::None => result,
    }
}

fn relevance_desc(left: &SearchResult, right: &SearchResult) -> Ordering {
    right.score.total_cmp(&left.score)
}

fn elapsed_ms(started: Instant) -> f64 {
    let micros = started.elapsed().as_micros() as f64;
    (micros / 1_000.0 * 1000.0).round() / 1000.0
}

/// Sort a result list by an explicit order.
pub fn sort_results(results: &mut [SearchResult], sort: Sort) {
    results.sort_by(|left, right| {
        let ordering = match sort.key {
            SortKey::Relevance => left.score.total_cmp(&right.score),
            SortKey::Name => compare_ci(&left.display_name, &right.display_name),
            SortKey::Path => compare_ci(
                left.path.as_deref().unwrap_or_default(),
                right.path.as_deref().unwrap_or_default(),
            ),
            SortKey::Modified => {
                compare_opt_i64(left.entity.modified_ms(), right.entity.modified_ms())
            }
            SortKey::Created => compare_opt_i64(created_ms(left), created_ms(right)),
            SortKey::Size => compare_u64(size_of(left), size_of(right)),
            SortKey::Pid => compare_opt_u32(pid_of(left), pid_of(right)),
            SortKey::Memory => compare_u64(memory_of(left), memory_of(right)),
        };
        match sort.direction {
            SortDirection::Asc => ordering,
            SortDirection::Desc => ordering.reverse(),
        }
    });
}

fn created_ms(result: &SearchResult) -> Option<i64> {
    match &result.entity {
        LocalEntity::File(entry) => entry.created,
        LocalEntity::Directory(entry) => entry.created,
        _ => None,
    }
}

fn size_of(result: &SearchResult) -> u64 {
    match &result.entity {
        LocalEntity::File(entry) => entry.size,
        _ => 0,
    }
}

fn pid_of(result: &SearchResult) -> Option<u32> {
    match &result.entity {
        LocalEntity::Process(entry) => Some(entry.pid),
        LocalEntity::Window(entry) => Some(entry.pid),
        _ => None,
    }
}

fn memory_of(result: &SearchResult) -> u64 {
    match &result.entity {
        LocalEntity::Process(entry) => entry.memory_bytes,
        _ => 0,
    }
}

fn compare_ci(left: &str, right: &str) -> Ordering {
    let left_lower = left.to_lowercase();
    let right_lower = right.to_lowercase();
    left_lower.cmp(&right_lower)
}

fn compare_opt_i64(left: Option<i64>, right: Option<i64>) -> Ordering {
    left.unwrap_or(i64::MIN).cmp(&right.unwrap_or(i64::MIN))
}

fn compare_opt_u32(left: Option<u32>, right: Option<u32>) -> Ordering {
    left.unwrap_or(u32::MAX).cmp(&right.unwrap_or(u32::MAX))
}

fn compare_u64(left: u64, right: u64) -> Ordering {
    left.cmp(&right)
}

#[derive(Debug)]
struct NoopRanker;

impl Ranker for NoopRanker {
    fn score(&self, _query: &SearchQuery, _entity: &LocalEntity) -> Option<RankedMatch> {
        Some(RankedMatch::bare(0.0))
    }
}

/// Build a default query the engine can run when a caller has nothing better.
#[must_use]
pub fn default_query() -> SearchQuery {
    SearchQuery {
        limit: DEFAULT_RESULT_LIMIT,
        ..SearchQuery::default()
    }
}

/// A ranker that keeps everything: useful in tests that exercise fan-out.
#[derive(Debug, Default)]
pub struct KeepAllRanker;

impl Ranker for KeepAllRanker {
    fn score(&self, _query: &SearchQuery, entity: &LocalEntity) -> Option<RankedMatch> {
        Some(RankedMatch::bare(recency_bonus(entity)))
    }
}

fn recency_bonus(entity: &LocalEntity) -> f32 {
    match entity.modified_ms() {
        Some(ms) => {
            let age_hours = (clock::now_ms() - ms).max(0) as f32 / 3_600_000.0;
            (30.0 - age_hours.min(30.0)).max(0.0)
        }
        None => 0.0,
    }
}

/// Unused entity type list, kept so the re-export surface stays explicit.
#[allow(dead_code)]
const _ALL_TYPES: [EntityType; 6] = EntityType::ALL;
