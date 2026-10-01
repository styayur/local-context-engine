//! # `query-dsl`
//!
//! Two ways to turn text into a [`SearchQuery`]:
//!
//! * [`dsl`] — a tiny, fully specified query language for power users and AI
//!   agents (`type:file ext:rs modified:<24h`).
//! * [`nl`] — a rule-based natural language compiler for humans
//!   (`最近的 pdf`, `正在运行的 python`, `running python processes`).
//!
//! Both are deterministic and offline. Neither depends on a model, a network
//! call or an embedding. That is the whole point of the design: an AI *may*
//! sit in front of the compiler later, but the compiler itself never needs one.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod compiler;
pub mod dsl;
pub mod nl;

pub use compiler::{
    compile, compile_detailed, CompileSource, CompiledQuery, QueryCompiler, RuleBasedCompiler,
};
pub use dsl::{parse, parse_or_plain, split_tokens, ParseOutcome};
pub use nl::compile_natural_language;

/// Re-exported so callers can build queries without depending on `search-core`
/// directly.
pub use search_core::{
    EntityType, Filter, SearchQuery, SizeFilter, SizeOp, Sort, SortKey, TimeBound,
};
