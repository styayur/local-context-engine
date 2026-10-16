//! Integration tests that talk to the real Windows APIs.
//!
//! These are the tests that would catch "the abstraction is fine but the
//! syscall is wrong". They deliberately assert *properties* rather than exact
//! values, so they pass on any Windows machine: the point is that the process
//! table is readable, that applications are discoverable and that an index of a
//! known directory can be searched.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use search_core::{EntityProvider as _, EntityType, Filter, LocalEntity, SearchQuery};
use windows_apps::AppProvider;
use windows_files::{scan::TempTree, FileProvider, IndexBackend, IndexConfig, VolumeSpec};
use windows_processes::ProcessProvider;
use windows_services::ServiceProvider;
use windows_windows::WindowProvider;

/// A file provider rooted at a temporary tree, so no test depends on the
/// developer machine's real layout.
fn provider_for(tree: &TempTree) -> FileProvider {
    let config = IndexConfig {
        volumes: vec![VolumeSpec::synthetic(
            tree.root().to_string_lossy().to_string(),
            'C',
            true,
        )],
        max_entries: 10_000,
        excluded_dir_names: IndexConfig::default_exclusions(),
        excluded_path_fragments: Vec::new(),
        include_directories: true,
    };
    let label = format!(
        "it-{}",
        tree.root()
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| std::process::id().to_string())
    );
    FileProvider::new(config, IndexBackend::Scan, label)
}

#[test]
fn process_listing_reports_the_test_process() {
    let provider = ProcessProvider::new();
    let entities = provider
        .collect(&SearchQuery::plain(""))
        .expect("the process table must be readable");

    assert!(!entities.is_empty(), "a running system has processes");

    let own_pid = std::process::id();
    let this_process = entities.iter().find_map(|entity| match entity {
        LocalEntity::Process(entry) if entry.pid == own_pid => Some(entry),
        _ => None,
    });
    let entry = this_process.expect("the test process must appear in its own snapshot");
    assert!(!entry.name.is_empty());
    assert!(entry.thread_count >= 1);
}

#[test]
fn process_results_are_live_snapshots_not_cached_forever() {
    let provider = ProcessProvider::new();
    let first = provider.collect(&SearchQuery::plain("")).unwrap().len();
    provider.invalidate();
    let second = provider.collect(&SearchQuery::plain("")).unwrap().len();
    // The count can change as processes start and stop, but it must never be
    // an empty list on a running system.
    assert!(first > 0 && second > 0);
}

#[test]
fn pid_filters_narrow_the_process_list() {
    let provider = ProcessProvider::new();
    let pid = std::process::id();
    let query = SearchQuery::plain("").with_filter(Filter::Pid(pid));
    let entities = provider.collect(&query).unwrap();
    assert_eq!(entities.len(), 1);
}

#[test]
fn application_discovery_finds_launchable_entries() {
    let provider = AppProvider::new();
    let entities = provider
        .collect(&SearchQuery::plain(""))
        .expect("application discovery must not fail");

    assert!(
        !entities.is_empty(),
        "a Windows install always exposes at least one executable on PATH"
    );
    for entity in &entities {
        match entity {
            LocalEntity::Application(entry) => {
                assert!(!entry.target.trim().is_empty(), "a target is mandatory");
                assert!(!entry.name.trim().is_empty(), "a name is mandatory");
            }
            other => panic!("the app provider returned a {other:?}"),
        }
    }
}

#[test]
fn application_search_matches_a_real_entry_by_name() {
    let provider = AppProvider::new();
    let all = provider.collect(&SearchQuery::plain("")).unwrap();
    let Some(first) = all.first() else {
        return;
    };
    let needle = first.name().to_string();
    let hits = provider
        .collect(&SearchQuery::plain(needle.clone()))
        .expect("searching the catalogue must not fail");
    assert!(
        hits.iter().any(|entity| entity.name() == needle),
        "searching for `{needle}` should find it again"
    );
}

#[test]
fn service_enumeration_is_read_only_and_complete() {
    let provider = ServiceProvider::new();
    let entities = provider
        .collect(&SearchQuery::plain(""))
        .expect("the service control manager must be readable");

    assert!(!entities.is_empty(), "every Windows install has services");
    for entity in &entities {
        match entity {
            LocalEntity::Service(entry) => assert!(!entry.name.is_empty()),
            other => panic!("the service provider returned a {other:?}"),
        }
    }

    let stats = provider.stats();
    assert_eq!(
        stats.detail.get("mode").map(String::as_str),
        Some("read-only")
    );
}

#[test]
fn file_index_basic_search_over_a_known_directory() {
    let tree = TempTree::new("integration").expect("temporary tree");
    tree.write(r"notes\alpha-report.pdf", b"alpha").unwrap();
    tree.write(r"notes\bravo-notes.txt", b"bravo").unwrap();
    tree.write(r"src\main.rs", b"fn main() {}").unwrap();
    tree.mkdir("empty-folder").unwrap();

    let provider = provider_for(&tree);
    let report = provider
        .rebuild()
        .expect("indexing a temp tree must succeed");
    assert_eq!(report.backend, "scan");
    assert!(report.entries >= 5, "got {} entries", report.entries);
    assert!(!report.truncated);

    // Name search.
    let hits = provider.collect(&SearchQuery::plain("alpha")).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name(), "alpha-report.pdf");

    // Extension filter.
    let hits = provider
        .collect(&SearchQuery::plain("").with_filter(Filter::Extension("rs".into())))
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name(), "main.rs");

    // Directory-only.
    let hits = provider
        .collect(&SearchQuery::plain("").with_types([EntityType::Directory]))
        .unwrap();
    assert!(hits.iter().any(|entity| entity.name() == "empty-folder"));

    // Multi-token AND.
    let hits = provider
        .collect(&SearchQuery::plain("alpha report"))
        .unwrap();
    assert_eq!(hits.len(), 1);

    provider.clear_cache().unwrap();
}

#[test]
fn an_indexed_tree_survives_a_restart() {
    let tree = TempTree::new("integration-persist").expect("temporary tree");
    tree.write("persisted.txt", b"x").unwrap();
    let label = format!("it-{}", tree.root().file_name().unwrap().to_string_lossy());

    let config = IndexConfig {
        volumes: vec![VolumeSpec::synthetic(
            tree.root().to_string_lossy().to_string(),
            'C',
            true,
        )],
        ..IndexConfig::default()
    };

    FileProvider::new(config, IndexBackend::Scan, label.clone())
        .rebuild()
        .unwrap();

    let reopened = FileProvider::new(IndexConfig::default(), IndexBackend::Scan, label.clone());
    assert!(reopened.is_ready(), "the cache should have been loaded");
    let hits = reopened.collect(&SearchQuery::plain("persisted")).unwrap();
    assert_eq!(hits.len(), 1);

    reopened.clear_cache().unwrap();
}

#[test]
fn window_enumeration_never_returns_an_untitled_window() {
    let provider = WindowProvider::new();
    let entities = provider
        .collect(&SearchQuery::plain(""))
        .expect("window enumeration must not fail");
    for entity in &entities {
        assert!(!entity.name().trim().is_empty());
    }
}
