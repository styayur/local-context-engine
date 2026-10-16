//! # `search-daemon`
//!
//! The composition root. Everything above this crate — the CLI, the MCP
//! server and the desktop UI — calls [`SearchService`] and nothing else.
//!
//! ```text
//!   CLI ─┐
//!   MCP ─┼─▶ SearchService ─▶ SearchEngine ─▶ providers ─▶ Win32
//!   UI  ─┘        │
//!                └─▶ RuleBasedCompiler ─▶ SearchQuery
//! ```

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod actions;
pub mod config;
pub mod service;
pub mod service_client;
pub mod usage_store;

pub use actions::OpenTarget;
pub use config::{settings_path, usage_path, Language, Settings, Theme};
pub use service::{IndexStatus, SearchOptions, SearchOutcome, SearchService};
pub use service_client::{is_available as index_service_available, ServiceStatus};

pub use query_dsl::{CompileSource, CompiledQuery, QueryCompiler};
pub use ranking::{RankingWeights, UsageEntry, UsageIndex};
pub use search_core::{
    EntityType, LceError, LocalEntity, SearchQuery, SearchResponse, SearchResult,
};
pub use windows_files::{IndexBackend, IndexReport};

/// The query-plan type the CLI's `--explain` prints.
pub mod windows_files_plan {
    pub use windows_files::QueryPlanInfo;
}
