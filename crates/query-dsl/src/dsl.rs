//! The Local Search DSL.
//!
//! The grammar is intentionally small enough to fit in your head:
//!
//! ```text
//! query   := (token)*
//! token   := key ":" value | word
//! key     := type | ext | path | drive | name | modified | created | size
//!          | state | pid | user | visible | sort | limit
//! ```
//!
//! Anything the parser does not recognise stays in the free-text portion of
//! the query, which is what makes `localsearch "C:\Users\me"` a perfectly
//! reasonable search instead of a parse error. Parsing never fails: it always
//! produces a usable [`SearchQuery`].

use std::time::Duration;

use search_core::clock;
use search_core::{
    EntityType, Filter, SearchQuery, SizeFilter, SizeOp, Sort, TimeBound, DEFAULT_RESULT_LIMIT,
};

/// What the parser made of an input string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseOutcome {
    /// The compiled query.
    pub query: SearchQuery,
    /// Whether at least one structured token was understood.
    pub structured: bool,
    /// Tokens that looked like `key:value` but used an unknown key. They are
    /// folded into the free-text part of the query.
    pub unknown_keys: Vec<String>,
    /// The free-text remainder, after structured tokens were removed.
    pub free_text: String,
}

/// Split an input line into tokens, honouring double quotes.
///
/// Quotes are preserved so that `path:"My Projects"` survives as one token;
/// [`unquote`] strips them once the key has been identified.
#[must_use]
pub fn split_tokens(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in input.chars() {
        if c == '"' {
            in_quotes = !in_quotes;
            current.push(c);
            continue;
        }
        if c.is_whitespace() && !in_quotes {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(c);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.replace('"', "")
    }
}

/// Parse a DSL string. Never fails; unrecognised input becomes free text.
#[must_use]
pub fn parse(input: &str) -> ParseOutcome {
    let mut query = SearchQuery {
        limit: DEFAULT_RESULT_LIMIT,
        ..SearchQuery::default()
    };
    let mut text_parts: Vec<String> = Vec::new();
    let mut structured = false;
    let mut unknown_keys = Vec::new();

    for token in split_tokens(input) {
        let Some((raw_key, raw_value)) = token.split_once(':') else {
            text_parts.push(unquote(&token));
            continue;
        };
        let key = raw_key.trim().to_ascii_lowercase();
        let value = unquote(raw_value);
        if value.is_empty() {
            text_parts.push(unquote(&token));
            continue;
        }

        let handled = match key.as_str() {
            "type" | "types" | "kind" => {
                let mut any = false;
                for part in value.split([',', '|']) {
                    if let Some(entity_type) = EntityType::parse(part) {
                        if !query.entity_types.contains(&entity_type) {
                            query.entity_types.push(entity_type);
                        }
                        any = true;
                    }
                }
                any
            }
            "ext" | "extension" | "filetype" => {
                value
                    .split([',', '|'])
                    .filter(|part| !part.trim().is_empty())
                    .map(|part| {
                        query.filters.push(Filter::Extension(
                            part.trim().trim_start_matches('.').to_string(),
                        ));
                    })
                    .count()
                    > 0
            }
            "path" | "in" => {
                query.filters.push(Filter::Path(value.clone()));
                true
            }
            "name" => {
                query.filters.push(Filter::Name(value.clone()));
                true
            }
            "prefix" | "starts" | "startswith" => {
                query.filters.push(Filter::NamePrefix(value.clone()));
                true
            }
            "drive" | "volume" => {
                if let Some(letter) = value.chars().next() {
                    query.filters.push(Filter::Drive(letter));
                    true
                } else {
                    false
                }
            }
            "modified" | "mtime" => parse_time_bound(&value)
                .map(|bound| {
                    query.filters.push(Filter::Modified(bound));
                })
                .is_some(),
            "created" | "ctime" => parse_time_bound(&value)
                .map(|bound| {
                    query.filters.push(Filter::Created(bound));
                })
                .is_some(),
            "size" => parse_size_filter(&value)
                .map(|size| {
                    query.filters.push(Filter::Size(size));
                })
                .is_some(),
            "state" | "status" => {
                query.filters.push(Filter::State(value.clone()));
                true
            }
            "pid" => value
                .parse::<u32>()
                .map(|pid| {
                    query.filters.push(Filter::Pid(pid));
                })
                .is_ok(),
            "user" | "owner" => {
                query.filters.push(Filter::User(value.clone()));
                true
            }
            "visible" | "visibility" => match value.to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" | "shown" => {
                    query.filters.push(Filter::Visible(true));
                    true
                }
                "false" | "no" | "0" | "hidden" => {
                    query.filters.push(Filter::Visible(false));
                    true
                }
                _ => false,
            },
            "sort" | "order" => Sort::parse(&value)
                .map(|sort| query.sort = Some(sort))
                .is_some(),
            "limit" | "top" => value
                .parse::<usize>()
                .map(|limit| query.limit = limit.clamp(1, search_core::MAX_RESULT_LIMIT))
                .is_ok(),
            "exact" | "fuzzy" => {
                // Reserved for a future match-mode switch. Accepting the key
                // keeps older query strings parseable instead of silently
                // turning them into free text.
                true
            }
            _ => false,
        };

        if handled {
            structured = true;
        } else if key.is_empty() {
            text_parts.push(unquote(&token));
        } else if is_probable_key(&key) {
            unknown_keys.push(token.clone());
            text_parts.push(unquote(&token));
        } else {
            // A bare `C:\path` or `12:30` style token: plain text.
            text_parts.push(unquote(&token));
        }
    }

    let free_text = text_parts.join(" ");
    let trimmed = free_text.trim();
    if !trimmed.is_empty() {
        query.text = Some(trimmed.to_string());
    }

    ParseOutcome {
        query,
        structured,
        unknown_keys,
        free_text,
    }
}

/// Parse, and fall back to a plain keyword query if nothing was structured.
#[must_use]
pub fn parse_or_plain(input: &str) -> SearchQuery {
    let outcome = parse(input);
    if outcome.query.is_empty() {
        SearchQuery::plain(input)
    } else {
        outcome.query
    }
}

fn is_probable_key(key: &str) -> bool {
    key.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
}

/// Parse the value half of a `modified:`/`created:` token.
///
/// * `<24h`  — newer than 24 hours ago
/// * `>24h`  — older than 24 hours ago
/// * `<7d`   — newer than 7 days ago
/// * `>=2024-01-01` — on or after an absolute date
/// * `<=2024-01-01` — on or before an absolute date
#[must_use]
pub fn parse_time_bound(value: &str) -> Option<TimeBound> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (operator, rest) = split_operator(trimmed);
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }

    let duration = clock::parse_duration(rest);
    let date = clock::parse_date(rest);

    match operator {
        "<" => duration
            .map(TimeBound::Within)
            .or_else(|| date.map(TimeBound::Before)),
        "<=" => date
            .map(TimeBound::Before)
            .or_else(|| duration.map(TimeBound::Within)),
        ">" => duration
            .map(TimeBound::OlderThan)
            .or_else(|| date.map(TimeBound::After)),
        ">=" => date
            .map(TimeBound::After)
            .or_else(|| duration.map(TimeBound::OlderThan)),
        "=" | "==" => date.map(TimeBound::After),
        _ => duration
            .map(TimeBound::Within)
            .or_else(|| date.map(TimeBound::After)),
    }
}

fn split_operator(value: &str) -> (&str, &str) {
    for operator in [">=", "<=", "==", ">", "<", "="] {
        if let Some(rest) = value.strip_prefix(operator) {
            return (operator, rest);
        }
    }
    ("", value)
}

/// Parse the value half of a `size:` token, for example `>10mb`, `<=512kb`.
#[must_use]
pub fn parse_size_filter(value: &str) -> Option<SizeFilter> {
    let trimmed = value.trim();
    let (operator, rest) = split_operator(trimmed);
    let bytes = clock::parse_size(rest.trim())?;
    let op = match operator {
        ">" => SizeOp::GreaterThan,
        ">=" => SizeOp::GreaterOrEqual,
        "<" => SizeOp::LessThan,
        "<=" => SizeOp::LessOrEqual,
        "=" | "==" => SizeOp::Equal,
        // A bare `size:10mb` reads naturally as "at least 10 MB".
        _ => SizeOp::GreaterOrEqual,
    };
    Some(SizeFilter { op, bytes })
}

/// Convenience helper used by tests and by `--explain` output.
#[must_use]
pub fn describe(input: &str) -> String {
    parse(input).query.to_dsl()
}

/// Duration helper re-exported for tests.
#[must_use]
pub fn hours(value: u64) -> Duration {
    Duration::from_secs(value * 3_600)
}

#[cfg(test)]
mod tests {
    use super::*;
    use search_core::SortKey;

    #[test]
    fn plain_word_becomes_text() {
        let outcome = parse("chrome");
        assert_eq!(outcome.query.text.as_deref(), Some("chrome"));
        assert!(!outcome.structured);
        assert!(outcome.query.entity_types.is_empty());
        assert!(outcome.query.filters.is_empty());
    }

    #[test]
    fn type_token_restricts_entity_types() {
        let outcome = parse("type:file rust");
        assert_eq!(outcome.query.entity_types, vec![EntityType::File]);
        assert_eq!(outcome.query.text.as_deref(), Some("rust"));
        assert!(outcome.structured);
    }

    #[test]
    fn folder_alias_maps_to_directory() {
        let outcome = parse("type:folder projects");
        assert_eq!(outcome.query.entity_types, vec![EntityType::Directory]);
    }

    #[test]
    fn multiple_types_are_accepted() {
        let outcome = parse("type:file,process python");
        assert_eq!(
            outcome.query.entity_types,
            vec![EntityType::File, EntityType::Process]
        );
    }

    #[test]
    fn extension_filter_strips_leading_dot() {
        let outcome = parse("type:file ext:.rs");
        assert_eq!(outcome.query.filters, vec![Filter::Extension("rs".into())]);
    }

    #[test]
    fn modified_within_parses_relative_duration() {
        let outcome = parse("type:file ext:pdf modified:<24h");
        assert_eq!(
            outcome.query.filters,
            vec![
                Filter::Extension("pdf".into()),
                Filter::Modified(TimeBound::Within(hours(24)))
            ]
        );
    }

    #[test]
    fn modified_after_parses_absolute_date() {
        let outcome = parse("modified:>=2024-01-01");
        let expected = clock::parse_date("2024-01-01").unwrap();
        assert_eq!(
            outcome.query.filters,
            vec![Filter::Modified(TimeBound::After(expected))]
        );
    }

    #[test]
    fn size_filter_parses_units() {
        let outcome = parse("size:>10mb");
        assert_eq!(
            outcome.query.filters,
            vec![Filter::Size(SizeFilter {
                op: SizeOp::GreaterThan,
                bytes: 10 * 1_048_576
            })]
        );
    }

    #[test]
    fn bare_size_defaults_to_at_least() {
        let outcome = parse("size:10mb");
        assert_eq!(
            outcome.query.filters,
            vec![Filter::Size(SizeFilter {
                op: SizeOp::GreaterOrEqual,
                bytes: 10 * 1_048_576
            })]
        );
    }

    #[test]
    fn sort_token_parses_key_and_direction() {
        let outcome = parse("drive:C ext:exe sort:size-desc");
        assert_eq!(
            outcome.query.sort,
            Some(Sort::new(SortKey::Size, search_core::SortDirection::Desc))
        );
        assert_eq!(outcome.query.filters[0], Filter::Drive('C'));
    }

    #[test]
    fn quoted_values_keep_their_spaces() {
        let outcome = parse(r#"path:"My Projects" ext:toml"#);
        assert_eq!(
            outcome.query.filters,
            vec![
                Filter::Path("My Projects".into()),
                Filter::Extension("toml".into())
            ]
        );
        assert_eq!(outcome.query.text, None);
    }

    #[test]
    fn unquoted_windows_path_is_treated_as_text() {
        let outcome = parse(r"C:\Users\me\Documents");
        assert_eq!(
            outcome.query.text.as_deref(),
            Some(r"C:\Users\me\Documents")
        );
        assert!(outcome.query.filters.is_empty());
    }

    #[test]
    fn unknown_key_falls_back_to_text_without_failing() {
        let outcome = parse("bogus:value rust");
        assert_eq!(outcome.unknown_keys, vec!["bogus:value".to_string()]);
        assert_eq!(outcome.query.text.as_deref(), Some("bogus:value rust"));
        assert!(!outcome.structured);
    }

    #[test]
    fn state_and_pid_tokens_are_structured() {
        let outcome = parse("type:service state:running");
        assert_eq!(outcome.query.filters, vec![Filter::State("running".into())]);
        let outcome = parse("type:process pid:4321");
        assert_eq!(outcome.query.filters, vec![Filter::Pid(4321)]);
    }

    #[test]
    fn limit_token_is_clamped() {
        let outcome = parse("limit:99999");
        assert_eq!(outcome.query.limit, search_core::MAX_RESULT_LIMIT);
        let outcome = parse("limit:0");
        assert_eq!(outcome.query.limit, 1);
    }

    #[test]
    fn every_documented_example_parses_into_something_structured() {
        let examples = [
            "chrome",
            "type:file rust",
            "type:file ext:rs",
            "type:file ext:pdf modified:<24h",
            "type:process python",
            "type:process name:node",
            "type:app vscode",
            "type:service state:running",
            "type:window github",
            "path:projects ext:toml",
            "drive:C ext:exe sort:size-desc",
        ];
        for example in examples {
            let outcome = parse(example);
            assert!(
                !outcome.query.is_empty(),
                "`{example}` parsed to an empty query"
            );
        }
    }

    #[test]
    fn query_renders_back_to_canonical_dsl() {
        let outcome = parse("type:file ext:rs modified:<24h sort:modified-desc limit:10");
        assert_eq!(
            outcome.query.to_dsl(),
            "type:file ext:rs modified:<24h sort:modified-desc limit:10"
        );
    }

    #[test]
    fn canonical_dsl_round_trips() {
        let original = "type:file ext:rs modified:<24h rust sort:modified-desc";
        let once = parse(original).query;
        let twice = parse(&once.to_dsl()).query;
        assert_eq!(once, twice);
    }
}
