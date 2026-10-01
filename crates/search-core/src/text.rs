//! Text normalisation, tokenisation and the fuzzy matcher.
//!
//! Everything here is deterministic and allocation-conscious: it runs on the
//! hot path of every search, so it works on `char` slices and avoids building
//! intermediate strings unless the caller asks for them.

use crate::result::MatchRange;

/// Lower-case a single character without the multi-character expansion that
/// `str::to_lowercase` can produce. Keeping a 1:1 mapping is what lets the
/// matcher report character positions that line up with the original text.
#[inline]
fn lower_char(c: char) -> char {
    let mut iter = c.to_lowercase();
    match (iter.next(), iter.next()) {
        (Some(lowered), None) => lowered,
        _ => c,
    }
}

/// Case-fold and trim a string for comparison.
#[must_use]
pub fn normalize_lower(value: &str) -> String {
    value.trim().to_lowercase()
}

/// Split text into comparison tokens.
///
/// Whitespace and the separators that show up in Windows paths, file names and
/// process names all act as boundaries, so `Code.exe` yields `code` and `exe`.
#[must_use]
pub fn tokenize(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in value.chars() {
        if is_token_separator(c) {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(lower_char(c));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[inline]
fn is_token_separator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '-' | '_'
                | '/'
                | '\\'
                | '.'
                | ','
                | ';'
                | ':'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '\''
                | '"'
                | '+'
                | '&'
                | '|'
                | '='
                | '@'
                | '#'
        )
}

#[inline]
fn is_word_boundary(c: char) -> bool {
    is_token_separator(c)
}

/// A successful subsequence match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// Normalised quality in `0..=200`; higher is a tighter match.
    pub score: u32,
    /// Character positions in the haystack that were matched.
    pub positions: Vec<usize>,
}

/// Case-insensitive subsequence match.
///
/// All needle characters must appear in `haystack` in order. The score rewards
/// contiguous runs, word-boundary starts and camel-case humps, and is damped by
/// the gaps in between. Returns `None` when the needle is not a subsequence.
#[must_use]
pub fn fuzzy_match(needle: &str, haystack: &str) -> Option<FuzzyMatch> {
    let needle_chars: Vec<char> = needle
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(lower_char)
        .collect();
    if needle_chars.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            positions: Vec::new(),
        });
    }

    let hay: Vec<char> = haystack.chars().collect();
    if hay.len() < needle_chars.len() {
        return None;
    }

    let mut positions = Vec::with_capacity(needle_chars.len());
    let mut cursor = 0usize;
    for wanted in &needle_chars {
        let mut found = None;
        while cursor < hay.len() {
            if lower_char(hay[cursor]) == *wanted {
                found = Some(cursor);
                break;
            }
            cursor += 1;
        }
        let index = found?;
        positions.push(index);
        cursor = index + 1;
    }

    let matched = positions.len() as i64;
    let mut score: i64 = matched * 12;
    let mut previous: Option<usize> = None;
    let mut gaps: i64 = 0;

    for &position in &positions {
        if let Some(prev) = previous {
            let gap = position.saturating_sub(prev).saturating_sub(1) as i64;
            gaps += gap;
            if gap == 0 {
                score += 8;
            } else {
                score -= (gap * 3).min(24);
            }
        }
        if position == 0 {
            score += 12;
        } else {
            let before = hay[position - 1];
            if is_word_boundary(before) {
                score += 10;
            } else if hay[position].is_uppercase() && before.is_lowercase() {
                // camelCase hump, eg. matching `VE` in `VsCode`.
                score += 6;
            }
        }
        previous = Some(position);
    }

    // Prefer matches that hug the beginning of the string.
    if let Some(&first) = positions.first() {
        score -= (first as i64 * 2).min(24);
    }

    // Density: a match spanning 40 characters to consume 3 is not convincing.
    let span = positions
        .last()
        .copied()
        .unwrap_or(0)
        .saturating_sub(positions.first().copied().unwrap_or(0))
        .saturating_add(1) as i64;
    let density = (matched * 100) / span.max(1);
    score += density / 4;

    // Length damping so short needles do not out-rank specific ones.
    score -= ((hay.len() as i64 - matched) / 8).min(16);

    let _ = gaps;
    let score = score.clamp(0, 200) as u32;
    Some(FuzzyMatch { score, positions })
}

/// Character ranges in `haystack` that should be highlighted for `needle`.
///
/// Contiguous substring matches produce a single ranged span; otherwise the
/// individual fuzzy positions are collapsed into runs.
#[must_use]
pub fn match_ranges(needle: &str, haystack: &str) -> Vec<MatchRange> {
    let trimmed = needle.trim();
    if trimmed.is_empty() || haystack.is_empty() {
        return Vec::new();
    }

    if let Some(range) = substring_range(trimmed, haystack) {
        return vec![range];
    }

    match fuzzy_match(trimmed, haystack) {
        Some(fuzzy) => collapse_runs(&fuzzy.positions),
        None => Vec::new(),
    }
}

/// Locate a case-insensitive substring, returning a character range.
#[must_use]
pub fn substring_range(needle: &str, haystack: &str) -> Option<MatchRange> {
    let needle_chars: Vec<char> = needle.chars().map(lower_char).collect();
    if needle_chars.is_empty() {
        return None;
    }
    let hay: Vec<char> = haystack.chars().collect();
    if hay.len() < needle_chars.len() {
        return None;
    }
    let lowered: Vec<char> = hay.iter().copied().map(lower_char).collect();
    let last_start = lowered.len() - needle_chars.len();
    for start in 0..=last_start {
        if lowered[start..start + needle_chars.len()] == needle_chars[..] {
            return Some(MatchRange {
                start,
                end: start + needle_chars.len(),
            });
        }
    }
    None
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

/// Score a case-insensitive substring match, higher for earlier and tighter
/// occurrences. Returns `None` when the needle is absent.
#[must_use]
pub fn substring_score(needle: &str, haystack: &str) -> Option<u32> {
    let range = substring_range(needle, haystack)?;
    let hay_len = haystack.chars().count().max(1);
    let position_penalty = ((range.start as u32) * 40) / hay_len as u32;
    Some(100u32.saturating_sub(position_penalty.min(60)))
}

/// Whether `needle` occurs as a whole token inside `haystack`.
#[must_use]
pub fn has_token(needle: &str, haystack: &str) -> bool {
    let wanted = normalize_lower(needle);
    if wanted.is_empty() {
        return false;
    }
    tokenize(haystack).contains(&wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_paths_and_extensions() {
        assert_eq!(
            tokenize(r"D:\Projects\Code.exe"),
            vec!["d", "projects", "code", "exe"]
        );
    }

    #[test]
    fn tokenize_keeps_unicode_letters_together() {
        assert_eq!(tokenize("最近的 报告.pdf"), vec!["最近的", "报告", "pdf"]);
    }

    #[test]
    fn fuzzy_requires_ordered_subsequence() {
        assert!(fuzzy_match("vsc", "Visual Studio Code").is_some());
        assert!(fuzzy_match("cv", "Visual Studio Code").is_none());
    }

    #[test]
    fn fuzzy_prefers_contiguous_matches() {
        let tight = fuzzy_match("code", "Code.exe").expect("match");
        let loose = fuzzy_match("code", "c-o-d-e-something").expect("match");
        assert!(
            tight.score > loose.score,
            "{} vs {}",
            tight.score,
            loose.score
        );
    }

    #[test]
    fn fuzzy_is_case_insensitive() {
        assert!(fuzzy_match("VSCODE", "vscode").is_some());
        assert!(fuzzy_match("windows", "WINDOWS").is_some());
    }

    #[test]
    fn fuzzy_handles_chinese_text() {
        let matched = fuzzy_match("最近", "最近修改的报告").expect("match");
        assert_eq!(matched.positions, vec![0, 1]);
    }

    #[test]
    fn match_ranges_returns_single_span_for_substring() {
        let ranges = match_ranges("studio", "Visual Studio Code");
        assert_eq!(ranges, vec![MatchRange { start: 7, end: 13 }]);
    }

    #[test]
    fn match_ranges_collapses_fuzzy_runs() {
        // The matcher is greedy: it takes the earliest occurrence of each
        // needle character, so `s` binds to `viSual` rather than `Studio`.
        let ranges = match_ranges("vsc", "Visual Studio Code");
        assert_eq!(
            ranges,
            vec![
                MatchRange { start: 0, end: 1 },
                MatchRange { start: 2, end: 3 },
                MatchRange { start: 14, end: 15 },
            ]
        );
    }

    #[test]
    fn fuzzy_positions_are_ordered_and_unique() {
        let matched = fuzzy_match("vsc", "Visual Studio Code").expect("match");
        assert!(matched.positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn substring_score_rewards_early_matches() {
        let early = substring_score("code", "Code.exe").expect("match");
        let late = substring_score("code", "something-code").expect("match");
        assert!(early > late);
    }

    #[test]
    fn has_token_matches_whole_words_only() {
        assert!(has_token("code", "Visual Studio Code"));
        assert!(!has_token("cod", "Visual Studio Code"));
    }
}
