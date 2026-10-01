//! # `search-core`
//!
//! The single source of truth for the Local Context Engine data model.
//!
//! Everything that can be searched is normalised into [`LocalEntity`], every
//! search goes through [`SearchQuery`], and every search produces a
//! [`SearchResponse`]. The CLI, the MCP server and the desktop UI all speak
//! this vocabulary, which is why none of them needs its own search
//! implementation.
//!
//! Layers above this crate (query compilation, ranking, platform providers)
//! depend on these types; this crate depends only on `serde`, `thiserror` and
//! `tracing`, so it stays cheap to compile and trivially testable.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod clock;
pub mod engine;
pub mod entity;
pub mod error;
pub mod matching;
pub mod provider;
pub mod query;
pub mod result;
pub mod text;

pub use clock::{format_size, format_unix_ms, human_size};
pub use engine::{default_query, RankedMatch, Ranker, SearchEngine};
pub use entity::{
    drive_of, extension_of, file_name_of, AppEntry, AppSource, DirectoryEntry, EntityType,
    FileEntry, LocalEntity, ProcessEntry, ServiceEntry, ServiceStartType, ServiceState,
    WindowEntry,
};
pub use error::{IndexError, LceError, PermissionError, PlatformError, SearchError};
pub use matching::{
    contains_ignore_case, passes_filters, requested_types, starts_with_ignore_case, wants_any,
};
pub use provider::{EntityProvider, ProviderStats, SnapshotScope};
pub use query::{Filter, SearchQuery, SizeFilter, SizeOp, Sort, SortDirection, SortKey, TimeBound};
pub use result::{
    MatchField, MatchRange, ProviderTiming, SearchResponse, SearchResult, DEFAULT_RESULT_LIMIT,
    MAX_RESULT_LIMIT,
};
pub use text::{fuzzy_match, match_ranges, substring_score, tokenize};

/// Crate-wide default result limit, re-exported for convenience.
pub const DEFAULT_LIMIT: usize = DEFAULT_RESULT_LIMIT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_ids_are_stable_and_case_insensitive() {
        let file = LocalEntity::File(FileEntry {
            path: r"D:\Projects\ReadMe.md".into(),
            name: "ReadMe.md".into(),
            extension: Some("md".into()),
            size: 12,
            modified: None,
            created: None,
            drive: Some('D'),
            file_id: None,
        });
        assert_eq!(file.id(), r"file:d:\projects\readme.md");
    }

    #[test]
    fn every_entity_type_round_trips_through_its_name() {
        for entity_type in EntityType::ALL {
            assert_eq!(EntityType::parse(entity_type.as_str()), Some(entity_type));
        }
    }

    #[test]
    fn extension_helpers_handle_paths_and_dots() {
        assert_eq!(extension_of(r"C:\a\main.rs"), Some("rs"));
        assert_eq!(extension_of(r"C:\a\no-extension"), None);
        assert_eq!(extension_of(r"C:\a\.gitignore"), None);
        assert_eq!(drive_of(r"D:\x"), Some('D'));
        assert_eq!(drive_of(r"\\server\share"), None);
        assert_eq!(file_name_of(r"C:\a\b\"), "b");
    }
}
