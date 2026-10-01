//! The query compiler pipeline.
//!
//! ```text
//! input ──▶ DSL parser ──▶ structured? ──yes──▶ SearchQuery
//!                              │no
//!                              ▼
//!                     rule-based NL compiler ──▶ matched? ──yes──▶ SearchQuery
//!                              │no
//!                              ▼
//!                        plain keyword query
//! ```
//!
//! Every stage is deterministic and runs locally in microseconds. This is the
//! seam an AI provider would plug into later — and the reason the search hot
//! path never has to wait for one.

use std::fmt;

use search_core::SearchQuery;

use crate::{dsl, nl};

/// Which stage produced the final query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileSource {
    /// The input contained at least one recognised DSL token.
    Dsl,
    /// The input matched a natural language rule.
    NaturalLanguage,
    /// The input was treated as plain keywords.
    PlainText,
}

impl CompileSource {
    /// Lower-case wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            CompileSource::Dsl => "dsl",
            CompileSource::NaturalLanguage => "natural-language",
            CompileSource::PlainText => "plain-text",
        }
    }
}

impl fmt::Display for CompileSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A compiled query plus the provenance the CLI's `--explain` flag prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledQuery {
    /// The untouched user input.
    pub raw: String,
    /// The structured query the engine will run.
    pub query: SearchQuery,
    /// Which stage produced it.
    pub source: CompileSource,
    /// Human-readable explanation notes.
    pub notes: Vec<String>,
}

impl CompiledQuery {
    /// The canonical DSL rendering of the compiled query.
    #[must_use]
    pub fn to_dsl(&self) -> String {
        self.query.to_dsl()
    }
}

/// Turns human input into a [`SearchQuery`].
///
/// The bundled implementation is [`RuleBasedCompiler`]. An AI-backed
/// implementation can be dropped in later without changing a single line of
/// the search engine.
pub trait QueryCompiler: Send + Sync + fmt::Debug {
    /// Compile `input` into a query.
    fn compile(&self, input: &str) -> SearchQuery {
        self.compile_detailed(input).query
    }

    /// Compile `input` and keep the explanation.
    fn compile_detailed(&self, input: &str) -> CompiledQuery;
}

/// The offline, rule-based compiler that ships with the MVP.
#[derive(Debug, Default, Clone, Copy)]
pub struct RuleBasedCompiler;

impl RuleBasedCompiler {
    /// Construct the compiler.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl QueryCompiler for RuleBasedCompiler {
    fn compile_detailed(&self, input: &str) -> CompiledQuery {
        let raw = input.trim().to_string();
        let mut notes: Vec<String> = Vec::new();

        let parsed = dsl::parse(&raw);
        if parsed.structured {
            if !parsed.unknown_keys.is_empty() {
                notes.push(format!(
                    "ignored unknown keys: {}",
                    parsed.unknown_keys.join(", ")
                ));
            }
            notes.push("source: dsl".into());
            return CompiledQuery {
                raw,
                query: parsed.query,
                source: CompileSource::Dsl,
                notes,
            };
        }

        let natural = nl::compile_natural_language(&raw);
        if natural.matched {
            notes.extend(natural.notes);
            notes.push("source: natural-language".into());
            return CompiledQuery {
                raw,
                query: natural.query,
                source: CompileSource::NaturalLanguage,
                notes,
            };
        }

        notes.push("source: plain-text".into());
        CompiledQuery {
            raw: raw.clone(),
            query: SearchQuery::plain(raw),
            source: CompileSource::PlainText,
            notes,
        }
    }
}

/// Compile a string with the default compiler.
#[must_use]
pub fn compile(input: &str) -> SearchQuery {
    RuleBasedCompiler.compile(input)
}

/// Compile a string and keep the provenance.
#[must_use]
pub fn compile_detailed(input: &str) -> CompiledQuery {
    RuleBasedCompiler.compile_detailed(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use search_core::{EntityType, Filter, Sort, TimeBound};

    #[test]
    fn dsl_wins_over_natural_language() {
        let compiled = compile_detailed("type:file ext:rs modified:<24h");
        assert_eq!(compiled.source, CompileSource::Dsl);
        assert_eq!(compiled.query.entity_types, vec![EntityType::File]);
        assert_eq!(compiled.query.text, None);
    }

    #[test]
    fn natural_language_is_used_when_there_is_no_dsl() {
        let compiled = compile_detailed("正在运行的 python");
        assert_eq!(compiled.source, CompileSource::NaturalLanguage);
        assert_eq!(compiled.query.entity_types, vec![EntityType::Process]);
        assert_eq!(compiled.query.text.as_deref(), Some("python"));
    }

    #[test]
    fn plain_keywords_fall_through_to_text() {
        let compiled = compile_detailed("visual studio code");
        assert_eq!(compiled.source, CompileSource::PlainText);
        assert_eq!(compiled.query.text.as_deref(), Some("visual studio code"));
        assert!(compiled.query.entity_types.is_empty());
    }

    #[test]
    fn empty_input_is_an_empty_query() {
        let compiled = compile_detailed("   ");
        assert!(compiled.query.is_empty());
        assert_eq!(compiled.query.limit, search_core::DEFAULT_RESULT_LIMIT);
    }

    #[test]
    fn compiled_query_has_a_canonical_dsl_rendering() {
        let compiled = compile_detailed("最近的 pdf");
        assert_eq!(compiled.to_dsl(), "type:file ext:pdf sort:modified-desc");
    }

    #[test]
    fn unknown_dsl_keys_are_reported_but_not_fatal() {
        let compiled = compile_detailed("colour:red type:file rust");
        assert_eq!(compiled.source, CompileSource::Dsl);
        assert!(compiled
            .notes
            .iter()
            .any(|note| note.contains("colour:red")));
        assert_eq!(compiled.query.text.as_deref(), Some("colour:red rust"));
    }

    #[test]
    fn compiler_trait_default_method_matches_free_function() {
        let compiler = RuleBasedCompiler::new();
        assert_eq!(
            QueryCompiler::compile(&compiler, "type:process node"),
            compile("type:process node")
        );
    }

    #[test]
    fn english_and_chinese_intents_compile_to_the_same_query() {
        let cases = [
            ("最近的 PDF", "recent PDFs"),
            ("正在运行的 python", "running python processes"),
            ("找 VS Code", "find VS Code"),
            ("昨天修改的 rust 文件", "yesterday modified rust files"),
        ];
        for (zh, en) in cases {
            assert_eq!(
                compile(zh),
                compile(en),
                "`{zh}` and `{en}` should compile identically"
            );
        }
    }

    #[test]
    fn sort_and_time_filters_survive_compilation() {
        let query = compile("type:file ext:pdf modified:<24h sort:size-desc");
        assert_eq!(
            query.sort,
            Some(Sort::new(
                search_core::SortKey::Size,
                search_core::SortDirection::Desc
            ))
        );
        assert!(query.filters.contains(&Filter::Modified(TimeBound::Within(
            std::time::Duration::from_secs(24 * 3_600)
        ))));
    }
}
