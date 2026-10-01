//! The heuristic ranker.
//!
//! Scoring is a pure function of the query and the entity (plus the optional
//! local usage index), which makes it trivial to unit test and impossible to
//! drift between the CLI, the MCP server and the desktop UI.

use std::sync::{Arc, RwLock};

use search_core::{
    clock, text, EntityType, LocalEntity, MatchField, MatchRange, RankedMatch, Ranker, SearchQuery,
};

use crate::usage::UsageIndex;
use crate::weights::RankingWeights;

/// A scored candidate with the arithmetic kept for `--explain`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreBreakdown {
    /// Final score.
    pub total: f32,
    /// Named components that produced `total`.
    pub components: Vec<(String, f32)>,
    /// Highlight ranges.
    pub ranges: Vec<MatchRange>,
    /// Which field the ranges refer to.
    pub field: MatchField,
}

impl ScoreBreakdown {
    /// Render the breakdown as `exact_match +100, usage_frequency +3.2`.
    #[must_use]
    pub fn explain(&self) -> String {
        self.components
            .iter()
            .map(|(name, value)| format!("{name} {value:+.1}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The final score, as the engine sees it.
    #[must_use]
    pub fn score(&self) -> f32 {
        self.total
    }

    fn into_ranked(self) -> RankedMatch {
        RankedMatch {
            score: self.total,
            field: self.field,
            ranges: self.ranges,
        }
    }
}

/// Deterministic heuristic ranking with optional usage boosting.
#[derive(Debug, Clone, Default)]
pub struct HeuristicRanker {
    weights: RankingWeights,
    usage: Option<Arc<RwLock<UsageIndex>>>,
}

impl HeuristicRanker {
    /// Build a ranker with explicit weights.
    #[must_use]
    pub fn new(weights: RankingWeights) -> Self {
        Self {
            weights,
            usage: None,
        }
    }

    /// Build a ranker that also consults a local usage index.
    ///
    /// The index is shared behind an `RwLock` because recording a selection
    /// has to be visible to the very next search, without rebuilding anything.
    #[must_use]
    pub fn with_usage(mut self, usage: Arc<RwLock<UsageIndex>>) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Replace the weights.
    pub fn set_weights(&mut self, weights: RankingWeights) {
        self.weights = weights;
    }

    /// The weights in use.
    #[must_use]
    pub const fn weights(&self) -> &RankingWeights {
        &self.weights
    }

    /// Score one candidate, keeping the component breakdown.
    #[must_use]
    pub fn score_detailed(
        &self,
        query: &SearchQuery,
        entity: &LocalEntity,
    ) -> Option<ScoreBreakdown> {
        let mut components: Vec<(String, f32)> = Vec::new();
        let mut total = 0.0f32;
        let mut field = MatchField::None;
        let mut ranges: Vec<MatchRange> = Vec::new();

        let text_query = query.text.as_deref().unwrap_or("").trim();
        let tokens: Vec<&str> = text_query.split_whitespace().collect();

        if !tokens.is_empty() {
            let name = entity.name();
            let path = entity.path().unwrap_or_default();
            let mut name_ranges: Vec<MatchRange> = Vec::new();
            let mut path_ranges: Vec<MatchRange> = Vec::new();
            let mut name_points = 0.0f32;
            let mut path_points = 0.0f32;

            for token in &tokens {
                if let Some(found) = self.score_field(token, name, true) {
                    name_points += found.points;
                    name_ranges.extend(found.ranges);
                } else if !path.is_empty() {
                    // Fuzzy matching is only meaningful against a short name.
                    // Paths are long enough that almost any needle is a
                    // subsequence of one, so they only get exact matching.
                    {
                        let found = self.score_field(token, path, false)?;
                        path_points += found.points * self.weights.path_only_factor;
                        path_ranges.extend(found.ranges);
                    }
                } else {
                    return None;
                }
            }

            if name_points > 0.0 {
                total += name_points;
                components.push(("name".into(), name_points));
                field = MatchField::Name;
                ranges = dedupe_ranges(name_ranges);
            }
            if path_points > 0.0 {
                total += path_points;
                components.push(("path".into(), path_points));
                if field == MatchField::None {
                    field = MatchField::Path;
                    ranges = dedupe_ranges(path_ranges);
                }
            }
        }

        if let Some(usage) = self.usage.as_ref() {
            let boost = usage
                .read()
                .map(|guard| guard.boost(&entity.id()))
                .unwrap_or(0.0);
            if boost > 0.0 {
                let points = boost * self.weights.usage_frequency;
                total += points;
                components.push(("usage_frequency".into(), points));
            }
        }

        if let Some(modified) = entity.modified_ms() {
            let factor = recency_factor(modified);
            if factor > 0.0 {
                let points = factor * self.weights.recency;
                total += points;
                components.push(("recency".into(), points));
            }
        }

        let preference = self
            .weights
            .entity_preference
            .for_type(entity.entity_type());
        if preference != 0.0 {
            total += preference;
            components.push(("entity_preference".into(), preference));
        }

        if entity.entity_type() == EntityType::Window && entity.name().trim().is_empty() {
            // An untitled top-level window is almost always noise.
            total -= 25.0;
            components.push(("untitled_window".into(), -25.0));
        }

        if total < self.weights.minimum_score {
            return None;
        }

        Some(ScoreBreakdown {
            total,
            components,
            ranges,
            field,
        })
    }

    /// Score one query token against one haystack.
    ///
    /// `allow_fuzzy` is false for paths, where the fuzzy fallback would match
    /// almost anything because of how long a path is.
    fn score_field(&self, needle: &str, haystack: &str, allow_fuzzy: bool) -> Option<FieldScore> {
        let needle = needle.trim();
        if needle.is_empty() || haystack.is_empty() {
            return None;
        }
        let lower_needle = needle.to_lowercase();
        let lower_haystack = haystack.to_lowercase();
        let needle_len = needle.chars().count();
        let haystack_len = haystack.chars().count();

        let exact = lower_needle == lower_haystack;
        let prefix = !exact && lower_haystack.starts_with(&lower_needle);

        let (mut points, mut ranges) = if exact {
            (
                self.weights.exact_match,
                vec![MatchRange {
                    start: 0,
                    end: haystack_len,
                }],
            )
        } else if prefix {
            (
                self.weights.prefix_match,
                vec![MatchRange {
                    start: 0,
                    end: needle_len,
                }],
            )
        } else if let Some(range) = text::substring_range(needle, haystack) {
            let position_penalty = (range.start as f32 / haystack_len.max(1) as f32)
                * self.weights.filename_match
                * 0.25;
            (
                (self.weights.filename_match - position_penalty).max(1.0),
                vec![range],
            )
        } else if allow_fuzzy && needle_len >= 2 {
            let fuzzy = text::fuzzy_match(needle, haystack)?;
            let bonus = fuzzy.score as f32 / 20.0;
            (
                self.weights.fuzzy_match + bonus,
                collapse_runs(&fuzzy.positions),
            )
        } else {
            return None;
        };

        if !exact && text::has_token(needle, haystack) {
            points += self.weights.token_match;
        }

        Some(FieldScore {
            points,
            ranges: dedupe_ranges(std::mem::take(&mut ranges)),
        })
    }
}

impl Ranker for HeuristicRanker {
    fn score(&self, query: &SearchQuery, entity: &LocalEntity) -> Option<RankedMatch> {
        self.score_detailed(query, entity)
            .map(ScoreBreakdown::into_ranked)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct FieldScore {
    points: f32,
    ranges: Vec<MatchRange>,
}

fn collapse_runs(positions: &[usize]) -> Vec<MatchRange> {
    let mut ranges: Vec<MatchRange> = Vec::new();
    for &position in positions {
        match ranges.last_mut() {
            Some(last) if last.end == position => last.end = position + 1,
            _ => ranges.push(MatchRange {
                start: position,
                end: position + 1,
            }),
        }
    }
    ranges
}

fn dedupe_ranges(mut ranges: Vec<MatchRange>) -> Vec<MatchRange> {
    ranges.retain(|range| range.end > range.start);
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<MatchRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => {
                last.end = last.end.max(range.end);
            }
            _ => merged.push(range),
        }
    }
    merged
}

/// Recency expressed as `0.0..=1.0`, with a 365 day horizon.
fn recency_factor(modified_ms: i64) -> f32 {
    let age_ms = (clock::now_ms() - modified_ms).max(0) as f64;
    let age_hours = age_ms / 3_600_000.0;
    if age_hours <= 1.0 {
        return 1.0;
    }
    let horizon: f64 = 24.0 * 365.0;
    let factor = 1.0 - ((1.0 + age_hours).ln() / (1.0 + horizon).ln());
    factor.clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use search_core::{AppEntry, AppSource, FileEntry, ProcessEntry};

    fn file(name: &str, path: &str, modified: Option<i64>) -> LocalEntity {
        LocalEntity::File(FileEntry {
            path: path.to_string(),
            name: name.to_string(),
            extension: search_core::extension_of(name).map(str::to_string),
            size: 1_024,
            modified,
            created: None,
            drive: path.chars().next(),
            file_id: None,
        })
    }

    fn app(name: &str, target: &str) -> LocalEntity {
        LocalEntity::Application(AppEntry {
            name: name.to_string(),
            target: target.to_string(),
            arguments: None,
            working_dir: None,
            source: AppSource::StartMenu,
        })
    }

    fn process(name: &str, pid: u32) -> LocalEntity {
        LocalEntity::Process(ProcessEntry {
            pid,
            name: name.to_string(),
            exe_path: Some(format!(r"C:\Program Files\{name}")),
            parent_pid: Some(1),
            memory_bytes: 64 * 1_048_576,
            start_time: Some(clock::now_ms()),
            username: None,
            thread_count: 8,
            session_id: Some(1),
        })
    }

    #[test]
    fn exact_match_beats_every_other_category() {
        let ranker = HeuristicRanker::default();
        let query = SearchQuery::plain("code");
        let exact = ranker
            .score_detailed(&query, &file("code", r"C:\code", None))
            .unwrap();
        let prefix = ranker
            .score_detailed(&query, &file("code.exe", r"C:\code.exe", None))
            .unwrap();
        assert!(exact.total > prefix.total);
    }

    #[test]
    fn prefix_beats_substring() {
        let ranker = HeuristicRanker::default();
        let query = SearchQuery::plain("code");
        let prefix = ranker
            .score_detailed(&query, &file("code.exe", r"C:\code.exe", None))
            .unwrap();
        let substring = ranker
            .score_detailed(&query, &file("mycode.txt", r"C:\mycode.txt", None))
            .unwrap();
        assert!(prefix.total > substring.total);
    }

    #[test]
    fn substring_beats_fuzzy() {
        let ranker = HeuristicRanker::default();
        let query = SearchQuery::plain("studio");
        let substring = ranker
            .score_detailed(&query, &file("Visual Studio Code.lnk", r"C:\x", None))
            .unwrap();
        let fuzzy = ranker
            .score_detailed(&query, &file("s-t-u-d-i-o-cafe.txt", r"C:\x", None))
            .unwrap();
        assert!(substring.total > fuzzy.total);
    }

    #[test]
    fn non_matching_entities_are_dropped() {
        let ranker = HeuristicRanker::default();
        let query = SearchQuery::plain("zzz");
        assert!(ranker
            .score_detailed(&query, &file("readme.md", r"C:\readme.md", None))
            .is_none());
    }

    #[test]
    fn path_match_alone_scores_lower_than_a_name_match() {
        let ranker = HeuristicRanker::default();
        let query = SearchQuery::plain("projects");
        let name_hit = ranker
            .score_detailed(&query, &file("projects.txt", r"C:\tmp\projects.txt", None))
            .unwrap();
        let path_hit = ranker
            .score_detailed(&query, &file("notes.txt", r"C:\projects\notes.txt", None))
            .unwrap();
        assert!(name_hit.total > path_hit.total);
    }

    #[test]
    fn every_query_token_must_match() {
        let ranker = HeuristicRanker::default();
        let query = SearchQuery::plain("visual code");
        assert!(ranker
            .score_detailed(&query, &app("Visual Studio Code", r"C:\Code.exe"))
            .is_some());
        // `code` appears neither in the name nor the path, so the whole
        // multi-token query must fail rather than degrade to a partial match.
        assert!(ranker
            .score_detailed(
                &query,
                &app("Visual Studio", r"C:\Program Files\Vendor\bin.exe")
            )
            .is_none());
    }

    #[test]
    fn applications_outrank_files_for_the_same_match() {
        let ranker = HeuristicRanker::default();
        let query = SearchQuery::plain("code");
        let application = ranker
            .score_detailed(&query, &app("Code", r"C:\Code.exe"))
            .unwrap();
        let file = ranker
            .score_detailed(&query, &file("Code", r"C:\Code", None))
            .unwrap();
        assert!(application.total > file.total);
    }

    #[test]
    fn recent_files_beat_stale_files_when_everything_else_is_equal() {
        let ranker = HeuristicRanker::default();
        let query = SearchQuery::plain("report");
        let now = clock::now_ms();
        let recent = ranker
            .score_detailed(&query, &file("report.pdf", r"C:\report.pdf", Some(now)))
            .unwrap();
        let stale = ranker
            .score_detailed(
                &query,
                &file(
                    "report.pdf",
                    r"C:\report.pdf",
                    Some(now - 400 * 24 * 3_600_000),
                ),
            )
            .unwrap();
        assert!(recent.total > stale.total);
    }

    #[test]
    fn usage_history_boosts_a_previously_selected_entity() {
        let mut usage = UsageIndex::default();
        for _ in 0..5 {
            usage.record("chrome", "app:c:\\chrome.exe");
        }
        let ranker = HeuristicRanker::default().with_usage(Arc::new(RwLock::new(usage)));
        let query = SearchQuery::plain("chrome");
        let boosted = ranker
            .score_detailed(&query, &app("Chrome", r"C:\chrome.exe"))
            .unwrap();
        let plain = HeuristicRanker::default()
            .score_detailed(&query, &app("Chrome", r"C:\chrome.exe"))
            .unwrap();
        assert!(boosted.total > plain.total);
    }

    #[test]
    fn usage_boost_is_bounded() {
        let mut usage = UsageIndex::default();
        for _ in 0..1_000 {
            usage.record("q", "file:c:\\x");
        }
        let ranker = HeuristicRanker::default().with_usage(Arc::new(RwLock::new(usage)));
        let query = SearchQuery::plain("x");
        let breakdown = ranker
            .score_detailed(&query, &file("x", r"C:\x", None))
            .unwrap();
        let usage_points = breakdown
            .components
            .iter()
            .find(|(name, _)| name == "usage_frequency")
            .map(|(_, value)| *value)
            .unwrap();
        assert!(usage_points <= 8.0 + f32::EPSILON, "was {usage_points}");
    }

    #[test]
    fn highlight_ranges_point_at_the_matched_substring() {
        let ranker = HeuristicRanker::default();
        let query = SearchQuery::plain("studio");
        let breakdown = ranker
            .score_detailed(&query, &app("Visual Studio Code", r"C:\Code.exe"))
            .unwrap();
        assert_eq!(breakdown.field, MatchField::Name);
        assert_eq!(breakdown.ranges, vec![MatchRange { start: 7, end: 13 }]);
    }

    #[test]
    fn case_insensitive_matching_works_for_chinese() {
        let ranker = HeuristicRanker::default();
        let query = SearchQuery::plain("报告");
        assert!(ranker
            .score_detailed(&query, &file("2024年度报告.pdf", r"C:\报告.pdf", None))
            .is_some());
    }

    #[test]
    fn process_search_matches_the_image_name() {
        let ranker = HeuristicRanker::default();
        let query = SearchQuery::plain("python");
        let breakdown = ranker
            .score_detailed(&query, &process("python.exe", 4242))
            .unwrap();
        assert_eq!(breakdown.field, MatchField::Name);
        assert!(breakdown.total > ranker.weights().exact_match / 2.0);
    }

    #[test]
    fn score_explanation_lists_the_components() {
        let ranker = HeuristicRanker::default();
        let query = SearchQuery::plain("code");
        let breakdown = ranker
            .score_detailed(&query, &file("code", r"C:\code", None))
            .unwrap();
        let explanation = breakdown.explain();
        assert!(explanation.contains("name"));
        assert!(explanation.contains("entity_preference"));
    }

    #[test]
    fn filter_only_queries_still_score() {
        let ranker = HeuristicRanker::default();
        let query = SearchQuery {
            text: None,
            ..SearchQuery::plain("")
        };
        assert!(ranker
            .score_detailed(&query, &process("node.exe", 1))
            .is_some());
    }
}
