//! The filesystem [`EntityProvider`].

use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

use search_core::{
    clock, contains_ignore_case, EntityProvider, EntityType, Filter, LceError, LocalEntity,
    ProviderStats, SearchQuery, SnapshotScope,
};

use crate::mft::{self, JournalState};
use crate::persist::{self, PersistedIndex};
use crate::scan::{self, IndexConfig, IndexReport};
use crate::store::{FileRecord, FileStore};
use crate::volume::{list_volumes, VolumeSpec};

/// Upper bound on candidates handed to the ranker for one query.
///
/// The ranker is cheap but not free, and a one million entry index must not
/// turn a keystroke into a million `LocalEntity` allocations.
pub const CANDIDATE_CAP: usize = 4_000;

/// Which index backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IndexBackend {
    /// Use the MFT backend when it is available and fall back to scanning.
    #[default]
    Auto,
    /// Always walk directories. Works everywhere, no elevation needed.
    Scan,
    /// Always use the NTFS MFT and change journal. Requires elevation.
    MftUsn,
}

impl std::str::FromStr for IndexBackend {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or(())
    }
}

impl IndexBackend {
    /// Parse a `--backend` value.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => IndexBackend::Auto,
            "scan" | "walk" | "dir" => IndexBackend::Scan,
            "mft" | "mft-usn" | "usn" | "ntfs" => IndexBackend::MftUsn,
            _ => return None,
        })
    }

    /// Lower-case wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            IndexBackend::Auto => "auto",
            IndexBackend::Scan => "scan",
            IndexBackend::MftUsn => "mft-usn",
        }
    }
}

#[derive(Debug)]
struct Runtime {
    store: FileStore,
    ready: bool,
    backend: String,
    volumes: Vec<String>,
    journal: Option<JournalState>,
    last_report: Option<IndexReport>,
}

/// Indexed filesystem provider.
#[derive(Debug)]
pub struct FileProvider {
    config: Mutex<IndexConfig>,
    requested_backend: IndexBackend,
    runtime: RwLock<Runtime>,
    cache_path: PathBuf,
    cache_label: String,
}

impl FileProvider {
    /// Build a provider for an explicit configuration.
    ///
    /// Any existing cache is loaded eagerly, so a warm start serves searches
    /// without touching the filesystem.
    #[must_use]
    pub fn new(config: IndexConfig, backend: IndexBackend, cache_label: impl Into<String>) -> Self {
        let cache_label = cache_label.into();
        let cache_path = persist::index_path(&cache_label);
        let mut runtime = Runtime {
            store: FileStore::new(),
            ready: false,
            backend: backend.as_str().to_string(),
            volumes: Vec::new(),
            journal: None,
            last_report: None,
        };

        if let Ok(Some(index)) = persist::load(&cache_path) {
            runtime.store = index.store;
            runtime.backend = index.backend;
            runtime.volumes = index.volumes;
            runtime.journal = index.journal;
            runtime.ready = true;
        }

        Self {
            config: Mutex::new(config),
            requested_backend: backend,
            runtime: RwLock::new(runtime),
            cache_path,
            cache_label,
        }
    }

    /// Build a provider that indexes every fixed local volume.
    #[must_use]
    pub fn with_defaults(backend: IndexBackend) -> Self {
        let config = IndexConfig {
            volumes: list_volumes(),
            ..IndexConfig::default()
        };
        Self::new(config, backend, "default")
    }

    /// The configuration in use.
    #[must_use]
    pub fn config(&self) -> IndexConfig {
        self.config
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Replace the configuration. The next rebuild uses it.
    pub fn set_config(&self, config: IndexConfig) {
        if let Ok(mut guard) = self.config.lock() {
            *guard = config;
        }
    }

    /// Which backend to prefer.
    #[must_use]
    pub const fn requested_backend(&self) -> IndexBackend {
        self.requested_backend
    }

    /// The backend that actually produced the current index.
    #[must_use]
    pub fn active_backend(&self) -> String {
        self.runtime
            .read()
            .map(|guard| guard.backend.clone())
            .unwrap_or_else(|_| "unknown".into())
    }

    /// Whether an index is loaded and searchable.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.runtime
            .read()
            .map(|guard| guard.ready)
            .unwrap_or(false)
    }

    /// How many entries are indexed.
    #[must_use]
    pub fn indexed_entries(&self) -> usize {
        self.runtime
            .read()
            .map(|guard| guard.store.len())
            .unwrap_or(0)
    }

    /// Approximate memory footprint of the index.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.runtime
            .read()
            .map(|guard| guard.store.memory_bytes())
            .unwrap_or(0)
    }

    /// The cache file backing this provider.
    #[must_use]
    pub fn cache_path(&self) -> &PathBuf {
        &self.cache_path
    }

    /// The last rebuild report, if the index has been built this session.
    #[must_use]
    pub fn last_report(&self) -> Option<IndexReport> {
        self.runtime
            .read()
            .ok()
            .and_then(|guard| guard.last_report.clone())
    }

    /// Build the index from scratch with the configured backend.
    pub fn rebuild(&self) -> Result<IndexReport, LceError> {
        let config = self
            .config
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let config = IndexConfig {
            volumes: if config.volumes.is_empty() {
                list_volumes()
            } else {
                config.volumes.clone()
            },
            ..config
        };

        let (store, journal, report) = self.build_store(&config)?;
        self.install(store, journal, &report);
        self.persist()?;
        Ok(report)
    }

    fn build_store(
        &self,
        config: &IndexConfig,
    ) -> Result<(FileStore, Option<JournalState>, IndexReport), LceError> {
        let want_mft = match self.requested_backend {
            IndexBackend::Scan => false,
            IndexBackend::MftUsn => true,
            IndexBackend::Auto => mft::is_available(&config.volumes),
        };

        if want_mft {
            match mft::build(config) {
                Ok(outcome) => return Ok((outcome.store, outcome.journal, outcome.report)),
                Err(error) => {
                    if self.requested_backend == IndexBackend::MftUsn {
                        return Err(error);
                    }
                    tracing::info!(
                        code = error.code(),
                        "falling back to the directory scan backend"
                    );
                }
            }
        }

        let (store, mut report) = scan::build_store(config);
        if self.requested_backend == IndexBackend::Auto && !config.volumes.is_empty() {
            report.warnings.push(
                "used the directory scan backend; run as administrator to index the NTFS master file table instead"
                    .into(),
            );
        }
        Ok((store, None, report))
    }

    fn install(&self, store: FileStore, journal: Option<JournalState>, report: &IndexReport) {
        if let Ok(mut guard) = self.runtime.write() {
            guard.store = store;
            guard.ready = true;
            guard.backend = report.backend.clone();
            guard.volumes = report.volumes.clone();
            guard.journal = journal;
            guard.last_report = Some(report.clone());
        }
    }

    fn persist(&self) -> Result<(), LceError> {
        let Some((store, journal, backend, volumes)) = self.runtime.read().ok().map(|guard| {
            (
                guard.store.clone(),
                guard.journal,
                guard.backend.clone(),
                guard.volumes.clone(),
            )
        }) else {
            return Ok(());
        };
        let index = PersistedIndex::new(backend, volumes, journal, store);
        persist::save(&self.cache_path, &index)
    }

    /// Apply incremental updates.
    ///
    /// Only the MFT backend has an incremental source, so this is a no-op for
    /// a scan-built index — which is honest: a directory scan has no journal to
    /// replay, and re-walking the disk on every refresh is exactly what the
    /// project exists to avoid.
    pub fn update(&self) -> Result<Option<mft::UpdateReport>, LceError> {
        let Some(state) = self.runtime.read().ok().and_then(|guard| guard.journal) else {
            return Ok(None);
        };
        let config = self.config();
        let backup = self
            .runtime
            .read()
            .map(|guard| guard.store.clone())
            .unwrap_or_default();

        let mut store = backup;
        let (report, next) = mft::update(&mut store, &config.volumes, state)?;
        if report.requires_rebuild {
            drop(store);
            let _ = self.rebuild()?;
            return Ok(Some(report));
        }
        if let Ok(mut guard) = self.runtime.write() {
            guard.store = store;
            guard.journal = Some(next);
            guard.last_report = Some(IndexReport {
                backend: "mft-usn".into(),
                volumes: guard.volumes.clone(),
                entries: guard.store.len(),
                directories: guard.store.directory_count(),
                truncated: false,
                elapsed_ms: 0.0,
                warnings: Vec::new(),
            });
        }
        self.persist()?;
        Ok(Some(report))
    }

    /// Delete the cache file so the next start rebuilds from scratch.
    pub fn clear_cache(&self) -> Result<(), LceError> {
        persist::remove(&self.cache_label)
    }

    /// The cache file for this provider.
    #[must_use]
    pub fn cache_label(&self) -> &str {
        &self.cache_label
    }

    fn state_is_ready(&self) -> bool {
        self.runtime
            .read()
            .map(|guard| guard.ready)
            .unwrap_or(false)
    }
}

impl EntityProvider for FileProvider {
    fn name(&self) -> &'static str {
        "files"
    }

    fn scope(&self) -> SnapshotScope {
        SnapshotScope::Cached
    }

    fn entity_types(&self) -> &'static [EntityType] {
        &[EntityType::File, EntityType::Directory]
    }

    fn collect(&self, query: &SearchQuery) -> Result<Vec<LocalEntity>, LceError> {
        if !query.accepts_type(EntityType::File) && !query.accepts_type(EntityType::Directory) {
            return Ok(Vec::new());
        }
        if !self.state_is_ready() {
            return Ok(Vec::new());
        }

        let Ok(guard) = self.runtime.read() else {
            return Ok(Vec::new());
        };
        Ok(collect_candidates(&guard.store, query, CANDIDATE_CAP))
    }
    fn stats(&self) -> ProviderStats {
        let ready = self.state_is_ready();
        let mut stats = ProviderStats::new(self.name(), self.scope(), ready);
        stats.entity_types = self.entity_types().to_vec();

        if let Ok(guard) = self.runtime.read() {
            stats.entity_count = Some(guard.store.len());
            stats.detail.insert("backend".into(), guard.backend.clone());
            stats
                .detail
                .insert("indexed_entries".into(), guard.store.len().to_string());
            stats
                .detail
                .insert("files".into(), guard.store.file_count().to_string());
            stats.detail.insert(
                "directories".into(),
                guard.store.directory_count().to_string(),
            );
            stats.detail.insert(
                "deleted_tombstones".into(),
                guard.store.deleted_count().to_string(),
            );
            stats.detail.insert(
                "memory_bytes".into(),
                guard.store.memory_bytes().to_string(),
            );
            stats
                .detail
                .insert("volumes".into(), guard.volumes.join(", "));
            if let Some(journal) = guard.journal {
                stats
                    .detail
                    .insert("journal_id".into(), journal.journal_id.to_string());
                stats
                    .detail
                    .insert("next_usn".into(), journal.next_usn.to_string());
            }
            if let Some(report) = &guard.last_report {
                stats.detail.insert(
                    "last_rebuild_ms".into(),
                    format!("{:.1}", report.elapsed_ms),
                );
                stats.warnings.extend(report.warnings.iter().cloned());
            }
            if !ready {
                stats.detail.insert("status".into(), "not-built".into());
            }
        }

        stats.detail.insert(
            "requested_backend".into(),
            self.requested_backend.as_str().into(),
        );
        stats.detail.insert(
            "cache_path".into(),
            self.cache_path.to_string_lossy().to_string(),
        );
        stats
    }

    fn refresh(&self) -> Result<(), LceError> {
        let _ = self.update()?;
        Ok(())
    }
}

/// The name-first candidate selection used by the index search.
///
/// Exposed so benchmarks and tests can exercise the exact code path the
/// provider uses, rather than a copy of it that could drift.
#[must_use]
pub fn collect_candidates(store: &FileStore, query: &SearchQuery, cap: usize) -> Vec<LocalEntity> {
    let wants_files = query.accepts_type(EntityType::File);
    let wants_directories = query.accepts_type(EntityType::Directory);
    if !wants_files && !wants_directories {
        return Vec::new();
    }

    let tokens: Vec<String> = query
        .text
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let path_filters: Vec<&str> = query
        .filters
        .iter()
        .filter_map(|filter| match filter {
            Filter::Path(needle) => Some(needle.as_str()),
            _ => None,
        })
        .collect();

    let mut candidates = Vec::new();
    for (offset, record) in store.records().iter().enumerate() {
        if candidates.len() >= cap {
            break;
        }
        if record.is_deleted() {
            continue;
        }
        if record.is_directory() {
            if !wants_directories {
                continue;
            }
        } else if !wants_files {
            continue;
        }
        let Ok(index) = u32::try_from(offset) else {
            break;
        };
        if !cheap_filters_ok(store, index, record, query) {
            continue;
        }

        if !tokens.is_empty() {
            let name = store.name(index);
            let all_in_name = tokens.iter().all(|token| contains_ignore_case(name, token));
            if !all_in_name {
                // A path hit is worth rebuilding the string, but only when the
                // name matched at least one token: otherwise a one million
                // entry store would build a million paths.
                let any_in_name = tokens.iter().any(|token| contains_ignore_case(name, token));
                if !any_in_name {
                    continue;
                }
                let path = store.path_of(index);
                if !tokens
                    .iter()
                    .all(|token| contains_ignore_case(&path, token))
                {
                    continue;
                }
            }
        }

        if !path_filters.is_empty() {
            let path = store.path_of(index);
            if !path_filters
                .iter()
                .all(|needle| contains_ignore_case(&path, needle))
            {
                continue;
            }
        }

        if let Some(entity) = store.entity(index) {
            candidates.push(entity);
        }
    }

    candidates
}
/// Structured predicates that can be decided without rebuilding the path.
fn cheap_filters_ok(
    store: &FileStore,
    index: u32,
    record: &FileRecord,
    query: &SearchQuery,
) -> bool {
    for filter in &query.filters {
        let ok = match filter {
            Filter::Extension(wanted) => {
                let wanted = wanted.trim_start_matches('.').to_ascii_lowercase();
                wanted.is_empty()
                    || (!record.is_directory()
                        && store
                            .extension(index)
                            .is_some_and(|ext| ext.eq_ignore_ascii_case(&wanted)))
            }
            Filter::Drive(letter) => {
                record.drive != 0 && char::from(record.drive) == letter.to_ascii_uppercase()
            }
            Filter::Size(size) => {
                if record.is_directory() {
                    false
                } else {
                    size.op.matches(record.size, size.bytes)
                }
            }
            Filter::Modified(bound) => bound.satisfied_by(nonzero(record.modified_ms)),
            Filter::Created(bound) => bound.satisfied_by(nonzero(record.created_ms)),
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    !record.is_deleted()
}

fn nonzero(value: i64) -> Option<i64> {
    (value != 0).then_some(value)
}

/// The volumes a fresh configuration would index.
#[must_use]
pub fn default_volumes() -> Vec<VolumeSpec> {
    list_volumes()
}

/// Describe the age of the on-disk cache, for `--index-status`.
#[must_use]
pub fn cache_age_ms(path: &std::path::Path) -> Option<i64> {
    let index = persist::load(path).ok().flatten()?;
    Some((clock::now_ms() - index.saved_at_ms).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::TempTree;

    fn provider_for(tree: &TempTree) -> FileProvider {
        let config = IndexConfig {
            volumes: vec![VolumeSpec {
                root: tree.root().to_string_lossy().to_string(),
                drive: 'C',
                file_system: "NTFS".into(),
                is_ntfs: true,
                total_bytes: None,
                free_bytes: None,
                needs_elevation: false,
            }],
            max_entries: 10_000,
            excluded_dir_names: IndexConfig::default_exclusions(),
            excluded_path_fragments: Vec::new(),
            include_directories: true,
        };
        // The cache label must be unique per test: tests run in parallel and
        // share one cache directory, so a fixed label would have them racing to
        // rename the same temporary file.
        let label = format!(
            "test-{}",
            tree.root()
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| std::process::id().to_string())
        );
        FileProvider::new(config, IndexBackend::Scan, label)
    }

    #[test]
    fn backend_parsing_accepts_the_documented_spellings() {
        assert_eq!(IndexBackend::parse("auto"), Some(IndexBackend::Auto));
        assert_eq!(IndexBackend::parse("scan"), Some(IndexBackend::Scan));
        assert_eq!(IndexBackend::parse("mft"), Some(IndexBackend::MftUsn));
        assert_eq!(IndexBackend::parse("MFT-USN"), Some(IndexBackend::MftUsn));
        assert_eq!(IndexBackend::parse("nonsense"), None);
    }

    #[test]
    fn the_provider_declares_files_and_directories() {
        let tree = TempTree::new("provider-types").unwrap();
        let provider = provider_for(&tree);
        assert_eq!(
            provider.entity_types(),
            &[EntityType::File, EntityType::Directory]
        );
        assert_eq!(provider.scope(), SnapshotScope::Cached);
    }

    #[test]
    fn an_unbuilt_provider_returns_no_results_but_still_reports_stats() {
        let tree = TempTree::new("provider-empty").unwrap();
        let provider = provider_for(&tree);
        provider.clear_cache().unwrap();
        let empty = FileProvider::new(IndexConfig::default(), IndexBackend::Scan, "test-unbuilt");
        empty.clear_cache().unwrap();
        assert!(empty.collect(&SearchQuery::plain("x")).unwrap().is_empty());
        assert!(!empty.stats().ready);
    }

    #[test]
    fn a_rebuild_makes_the_index_searchable() {
        let tree = TempTree::new("provider-rebuild").unwrap();
        tree.write("notes/alpha.txt", b"alpha").unwrap();
        tree.write("notes/bravo.pdf", b"bravo").unwrap();
        let provider = provider_for(&tree);

        let report = provider.rebuild().unwrap();
        assert_eq!(report.backend, "scan");
        assert!(provider.is_ready());

        let hits = provider.collect(&SearchQuery::plain("alpha")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name(), "alpha.txt");
    }

    #[test]
    fn search_is_case_insensitive() {
        let tree = TempTree::new("provider-case").unwrap();
        tree.write("ReadMe.MD", b"x").unwrap();
        let provider = provider_for(&tree);
        provider.rebuild().unwrap();
        assert_eq!(
            provider
                .collect(&SearchQuery::plain("readme"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            provider
                .collect(&SearchQuery::plain("README"))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn extension_filters_restrict_the_result_set() {
        let tree = TempTree::new("provider-ext").unwrap();
        tree.write("a.pdf", b"x").unwrap();
        tree.write("b.rs", b"x").unwrap();
        let provider = provider_for(&tree);
        provider.rebuild().unwrap();

        let query = SearchQuery::plain("").with_filter(Filter::Extension("pdf".into()));
        let hits = provider.collect(&query).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name(), "a.pdf");
    }

    #[test]
    fn directory_only_queries_skip_files() {
        let tree = TempTree::new("provider-dirs").unwrap();
        tree.write(r"projects\code.rs", b"x").unwrap();
        let provider = provider_for(&tree);
        provider.rebuild().unwrap();

        let query = SearchQuery::plain("").with_types([EntityType::Directory]);
        let hits = provider.collect(&query).unwrap();
        assert!(!hits.is_empty());
        assert!(hits
            .iter()
            .all(|entity| entity.entity_type() == EntityType::Directory));
    }

    #[test]
    fn path_filters_are_applied() {
        let tree = TempTree::new("provider-path").unwrap();
        tree.write(r"keep\visible.txt", b"x").unwrap();
        tree.write(r"other\hidden.txt", b"x").unwrap();
        let provider = provider_for(&tree);
        provider.rebuild().unwrap();

        let query = SearchQuery::plain("").with_filter(Filter::Path("keep".into()));
        let hits = provider.collect(&query).unwrap();
        assert!(hits.iter().all(|entity| entity.name() != "hidden.txt"));
    }

    #[test]
    fn multi_token_queries_require_every_token() {
        let tree = TempTree::new("provider-tokens").unwrap();
        tree.write("visual-studio-code.txt", b"x").unwrap();
        tree.write("visual-studio.txt", b"x").unwrap();
        let provider = provider_for(&tree);
        provider.rebuild().unwrap();

        let hits = provider
            .collect(&SearchQuery::plain("studio code"))
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name(), "visual-studio-code.txt");
    }

    #[test]
    fn size_filters_use_the_recorded_size() {
        let tree = TempTree::new("provider-size").unwrap();
        tree.write("small.txt", &[0u8; 16]).unwrap();
        tree.write("large.txt", &[0u8; 8192]).unwrap();
        let provider = provider_for(&tree);
        provider.rebuild().unwrap();

        let query = SearchQuery::plain("").with_filter(Filter::Size(search_core::SizeFilter {
            op: search_core::SizeOp::GreaterThan,
            bytes: 1_024,
        }));
        let hits = provider.collect(&query).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name(), "large.txt");
    }

    #[test]
    fn the_index_survives_a_restart_through_the_cache() {
        let tree = TempTree::new("provider-cache").unwrap();
        tree.write("persisted.txt", b"x").unwrap();
        let label = format!("test-cache-{}", std::process::id());

        let provider = {
            let config = IndexConfig {
                volumes: vec![VolumeSpec {
                    root: tree.root().to_string_lossy().to_string(),
                    drive: 'C',
                    file_system: "NTFS".into(),
                    is_ntfs: true,
                    total_bytes: None,
                    free_bytes: None,
                    needs_elevation: false,
                }],
                ..IndexConfig::default()
            };
            FileProvider::new(config.clone(), IndexBackend::Scan, label.clone())
        };
        provider.rebuild().unwrap();

        let reopened = FileProvider::new(IndexConfig::default(), IndexBackend::Scan, label.clone());
        assert!(reopened.is_ready(), "the cache should have been loaded");
        assert_eq!(
            reopened
                .collect(&SearchQuery::plain("persisted"))
                .unwrap()
                .len(),
            1
        );

        reopened.clear_cache().unwrap();
    }

    #[test]
    fn stats_expose_the_backend_and_entry_counts() {
        let tree = TempTree::new("provider-stats").unwrap();
        tree.write("counted.txt", b"x").unwrap();
        let provider = provider_for(&tree);
        provider.rebuild().unwrap();

        let stats = provider.stats();
        assert!(stats.ready);
        assert_eq!(
            stats.detail.get("backend").map(String::as_str),
            Some("scan")
        );
        assert!(stats.entity_count.unwrap_or(0) >= 2);
    }

    #[test]
    fn the_candidate_cap_bounds_a_huge_result_set() {
        let tree = TempTree::new("provider-cap").unwrap();
        for index in 0..32 {
            tree.write(&format!("match-{index}.txt"), b"x").unwrap();
        }
        let provider = provider_for(&tree);
        provider.rebuild().unwrap();
        let hits = provider.collect(&SearchQuery::plain("match")).unwrap();
        assert!(hits.len() <= CANDIDATE_CAP);
        assert_eq!(hits.len(), 32);
    }
}
