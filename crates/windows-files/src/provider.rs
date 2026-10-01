//! The filesystem [`EntityProvider`].
//!
//! One provider owns the index, its accelerators and its per-volume journal
//! cursors. Searches take a read lock; a journal batch is applied under a short
//! write lock; the disk I/O and parsing that produced that batch happened
//! outside any lock at all.

use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

use search_core::{
    clock, contains_ignore_case, starts_with_ignore_case, EntityProvider, EntityType, Filter,
    LceError, LocalEntity, ProviderStats, SearchQuery, SnapshotScope,
};

use crate::accelerators::{normalise_key, RecordId, SearchAccelerators};
use crate::journal::JournalRegistry;
use crate::mft;
use crate::persist::{self, PersistedIndex};
use crate::planner::{self, CandidateSource, QueryPlanInfo, SearchPlan};
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

/// The index and everything derived from it.
#[derive(Debug)]
struct IndexState {
    store: FileStore,
    accelerators: SearchAccelerators,
}

impl IndexState {
    fn empty() -> Self {
        Self {
            store: FileStore::new(),
            accelerators: SearchAccelerators::default(),
        }
    }

    fn from_store(store: FileStore) -> Self {
        let accelerators = SearchAccelerators::build(&store);
        Self {
            store,
            accelerators,
        }
    }
}

#[derive(Debug)]
struct Runtime {
    index: IndexState,
    ready: bool,
    backend: String,
    volumes: Vec<String>,
    journals: JournalRegistry,
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
            index: IndexState::empty(),
            ready: false,
            backend: backend.as_str().to_string(),
            volumes: Vec::new(),
            journals: JournalRegistry::new(),
            last_report: None,
        };

        if let Ok(Some(index)) = persist::load(&cache_path) {
            let backend = index.backend.clone();
            let volumes = index.volumes.clone();
            let (store, accelerators, journals) = index.into_parts();
            runtime.index = IndexState {
                store,
                accelerators,
            };
            runtime.backend = backend;
            runtime.volumes = volumes;
            runtime.journals = journals;
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
            .map(|guard| guard.index.store.len())
            .unwrap_or(0)
    }

    /// Approximate memory footprint of the index and its accelerators.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.runtime
            .read()
            .map(|guard| guard.index.store.memory_bytes() + guard.index.accelerators.memory_bytes())
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

    /// A snapshot of the per-volume journal state.
    #[must_use]
    pub fn journals(&self) -> JournalRegistry {
        self.runtime
            .read()
            .map(|guard| guard.journals.clone())
            .unwrap_or_default()
    }

    /// Accelerator statistics, for the index panel.
    #[must_use]
    pub fn accelerator_stats(&self) -> crate::accelerators::AcceleratorStats {
        self.runtime
            .read()
            .map(|guard| guard.index.accelerators.stats())
            .unwrap_or_default()
    }

    /// Plan a query without running it.
    ///
    /// This is a pure function of the query and the current index, which is
    /// what makes `--explain` deterministic rather than a race with whatever
    /// search happens to be running.
    #[must_use]
    pub fn explain(&self, query: &SearchQuery) -> QueryPlanInfo {
        let Ok(guard) = self.runtime.read() else {
            return planner::linear_plan("the index lock was poisoned");
        };
        if !guard.ready {
            return planner::linear_plan("the file index has not been built yet");
        }
        planner::plan(&guard.index.store, &guard.index.accelerators, query).info
    }

    /// Volumes currently configured for indexing.
    #[must_use]
    pub fn volumes(&self) -> Vec<VolumeSpec> {
        self.config().volumes
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

        let (store, journals, report) = self.build_store(&config)?;
        self.install(store, journals, &report);
        self.persist()?;
        Ok(report)
    }

    fn build_store(
        &self,
        config: &IndexConfig,
    ) -> Result<(FileStore, JournalRegistry, IndexReport), LceError> {
        let want_mft = match self.requested_backend {
            IndexBackend::Scan => false,
            IndexBackend::MftUsn => true,
            IndexBackend::Auto => mft::is_available(&config.volumes),
        };

        if want_mft {
            match mft::build(config) {
                Ok(outcome) => return Ok((outcome.store, outcome.journals, outcome.report)),
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
        Ok((store, JournalRegistry::new(), report))
    }

    fn install(&self, store: FileStore, journals: JournalRegistry, report: &IndexReport) {
        let index = IndexState::from_store(store);
        if let Ok(mut guard) = self.runtime.write() {
            guard.index = index;
            guard.ready = true;
            guard.backend = report.backend.clone();
            guard.volumes = report.volumes.clone();
            guard.journals = journals;
            guard.last_report = Some(report.clone());
        }
    }

    fn persist(&self) -> Result<(), LceError> {
        let Some((store, accelerators, journals, backend, volumes)) =
            self.runtime.read().ok().map(|guard| {
                (
                    guard.index.store.clone(),
                    guard.index.accelerators.clone(),
                    guard.journals.clone(),
                    guard.backend.clone(),
                    guard.volumes.clone(),
                )
            })
        else {
            return Ok(());
        };
        let index = PersistedIndex::new(backend, volumes, journals, store, accelerators);
        persist::save(&self.cache_path, &index)
    }

    /// Apply a batch of index mutations under one short write lock.
    ///
    /// The caller is expected to have produced `mutations` outside the lock —
    /// see [`mft::read_journal_batches`]. The accelerators are updated in the
    /// same critical section, so a search can never observe a record the
    /// accelerators do not know about, or the other way round.
    pub fn apply_mutations(
        &self,
        mutations: &[crate::mutation::IndexMutation],
    ) -> crate::mutation::ApplyReport {
        let Ok(mut guard) = self.runtime.write() else {
            return crate::mutation::ApplyReport::default();
        };
        let before = guard.index.store.len();
        let report = guard.index.store.apply_batch(mutations);

        // Newly appended records need indexing. A rename keeps its record index
        // but changes its name, so the new tokens are appended there too;
        // verification discards the stale ones.
        let mut to_index: Vec<RecordId> = Vec::new();
        for index in before..guard.index.store.len() {
            if let Ok(id) = RecordId::try_from(index) {
                to_index.push(id);
            }
        }
        for mutation in mutations {
            if let crate::mutation::IndexMutation::Rename { file_id, .. } = mutation {
                if let Some(id) = guard.index.store.find_by_file_id(*file_id) {
                    to_index.push(id);
                }
            }
        }
        for id in to_index {
            let name = guard.index.store.name(id).to_string();
            let extension = guard.index.store.extension(id).unwrap_or("").to_string();
            let path = guard.index.store.path_of(id);
            guard
                .index
                .accelerators
                .insert_indexed(&extension, &name, &path, id);
        }
        guard.index.accelerators.note_mutations(report.applied());
        report
    }

    /// Compact the accelerators if they have drifted far enough to matter.
    ///
    /// Returns whether a compaction happened, so the caller can log or
    /// checkpoint it.
    pub fn compact_accelerators_if_needed(&self) -> bool {
        let Ok(mut guard) = self.runtime.write() else {
            return false;
        };
        if !guard
            .index
            .accelerators
            .needs_compaction(guard.index.store.len())
        {
            return false;
        }
        guard.index.accelerators = SearchAccelerators::build(&guard.index.store);
        true
    }

    /// Apply incremental changes to the index.
    ///
    /// Three phases, in this order and no other:
    ///
    /// 1. read the journals and parse them into mutations — **no lock**;
    /// 2. take the write lock and apply the batch — short, in-memory only;
    /// 3. persist the cache — **no lock**.
    ///
    /// A scan-built index has no journal, so this reports "nothing to do"
    /// rather than pretending it refreshed something.
    pub fn update(&self) -> Result<Option<mft::UpdateReport>, LceError> {
        let config = self.config();
        if config.volumes.is_empty() {
            return Ok(None);
        }

        let snapshot = self
            .runtime
            .read()
            .map(|guard| guard.journals.clone())
            .unwrap_or_default();
        if snapshot.is_empty() {
            return Ok(None);
        }

        // Phase 1: disk I/O and parsing, with no lock held.
        let mut journals = snapshot;
        let batch = mft::read_journal_batches(
            &config.volumes,
            &mut journals,
            mft::JournalReadOptions::default(),
        );
        if batch.is_noop() {
            if let Ok(mut guard) = self.runtime.write() {
                guard.journals = journals;
            }
            return Ok(Some(mft::UpdateReport::default()));
        }

        // Phase 2: a short write lock that only touches memory.
        let report = {
            let Ok(mut guard) = self.runtime.write() else {
                return Ok(None);
            };
            let mut store = std::mem::take(&mut guard.index.store);
            let before = store.len();
            let report = mft::apply_journal_batch(&mut store, &mut journals, &batch);

            for index in before..store.len() {
                if let Ok(id) = RecordId::try_from(index) {
                    let name = store.name(id).to_string();
                    let extension = store.extension(id).unwrap_or("").to_string();
                    let path = store.path_of(id);
                    guard
                        .index
                        .accelerators
                        .insert_indexed(&extension, &name, &path, id);
                }
            }
            guard
                .index
                .accelerators
                .note_mutations(report.applied.applied());
            guard.index.store = store;
            guard.journals = journals;
            report
        };

        // Phase 3: persist without holding the lock.
        self.persist()?;
        Ok(Some(report))
    }
    /// Tail one volume's change journal once.
    ///
    /// This is the worker's whole loop body, and it is the three-phase pattern
    /// in miniature:
    ///
    /// 1. snapshot the journal cursors (read lock, then released);
    /// 2. **block in the kernel** inside `FSCTL_READ_USN_JOURNAL` — no lock is
    ///    held while waiting, so searches keep running;
    /// 3. take the write lock only to apply the batch, then persist without it.
    ///
    /// `options.wait` is what makes this event-driven: with a `Timeout` and
    /// `BytesToWaitFor` the call returns as soon as the journal moves instead
    /// of spinning.
    pub fn poll_volume(
        &self,
        volume: &VolumeSpec,
        options: mft::JournalReadOptions,
    ) -> Result<mft::UpdateReport, LceError> {
        // Phase 0: is this volume even ours to poll?
        let mut journals = self
            .runtime
            .read()
            .map(|guard| guard.journals.clone())
            .unwrap_or_default();

        // Phase 1: disk I/O and parsing, no lock held.
        let batch = mft::read_journal_batches(std::slice::from_ref(volume), &mut journals, options);

        // Phase 2: a short write lock that only touches memory.
        let (report, applied) = {
            let Ok(mut guard) = self.runtime.write() else {
                return Err(LceError::Io {
                    action: "applying a journal batch".into(),
                    detail: "the index lock was poisoned".into(),
                });
            };
            let mut store = std::mem::take(&mut guard.index.store);
            let before = store.len();
            let report = mft::apply_journal_batch(&mut store, &mut journals, &batch);

            for index in before..store.len() {
                if let Ok(id) = RecordId::try_from(index) {
                    let name = store.name(id).to_string();
                    let extension = store.extension(id).unwrap_or("").to_string();
                    let path = store.path_of(id);
                    guard
                        .index
                        .accelerators
                        .insert_indexed(&extension, &name, &path, id);
                }
            }
            guard
                .index
                .accelerators
                .note_mutations(report.applied.applied());
            guard.index.store = store;
            guard.journals = journals;
            guard.ready = true;
            let changed = report.applied.applied();
            (report, changed)
        };

        // Phase 3: persist without holding the lock, and only when something
        // actually changed.
        if applied > 0 {
            self.persist()?;
        }
        Ok(report)
    }

    /// Forget every cursor so the next poll rebuilds each volume.
    pub fn require_rebuild(&self, reason: crate::journal::RebuildReason) {
        if let Ok(mut guard) = self.runtime.write() {
            guard.journals.invalidate_all(reason);
        }
    }

    /// The journal state of one volume.
    #[must_use]
    pub fn volume_journal(
        &self,
        volume_id: &crate::volume::VolumeId,
    ) -> Option<crate::journal::VolumeJournalState> {
        self.runtime
            .read()
            .ok()
            .and_then(|guard| guard.journals.get(volume_id).cloned())
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
        let Ok(guard) = self.runtime.read() else {
            return Ok(Vec::new());
        };
        if !guard.ready {
            return Ok(Vec::new());
        }
        let (entities, _info) = collect_candidates_with_plan(
            &guard.index.store,
            &guard.index.accelerators,
            query,
            CANDIDATE_CAP,
        );
        Ok(entities)
    }

    fn stats(&self) -> ProviderStats {
        let ready = self.state_is_ready();
        let mut stats = ProviderStats::new(self.name(), self.scope(), ready);
        stats.entity_types = self.entity_types().to_vec();

        if let Ok(guard) = self.runtime.read() {
            let store = &guard.index.store;
            stats.entity_count = Some(store.len());
            stats.detail.insert("backend".into(), guard.backend.clone());
            stats
                .detail
                .insert("indexed_entries".into(), store.len().to_string());
            stats
                .detail
                .insert("files".into(), store.file_count().to_string());
            stats
                .detail
                .insert("directories".into(), store.directory_count().to_string());
            stats.detail.insert(
                "deleted_tombstones".into(),
                store.deleted_count().to_string(),
            );
            stats.detail.insert(
                "store_memory_bytes".into(),
                store.memory_bytes().to_string(),
            );

            let accelerator_stats = guard.index.accelerators.stats();
            stats.detail.insert(
                "accelerator_memory_bytes".into(),
                accelerator_stats.memory_bytes.to_string(),
            );
            stats.detail.insert(
                "extensions_indexed".into(),
                accelerator_stats.extensions.to_string(),
            );
            stats.detail.insert(
                "trigrams_indexed".into(),
                accelerator_stats.trigrams.to_string(),
            );
            stats.detail.insert(
                "trigrams_dropped".into(),
                accelerator_stats.trigrams_dropped.to_string(),
            );
            stats.detail.insert(
                "accelerator_drift".into(),
                guard.index.accelerators.dirty().to_string(),
            );
            stats.detail.insert(
                "memory_bytes".into(),
                (store.memory_bytes() + guard.index.accelerators.memory_bytes()).to_string(),
            );

            stats
                .detail
                .insert("volumes".into(), guard.volumes.join(", "));

            for (volume_key, state) in &guard.journals.volumes {
                let volume_id = crate::volume::VolumeId::new(volume_key.clone());
                stats.detail.insert(
                    format!("journal.{}.status", volume_id.short()),
                    state.status.as_str().to_string(),
                );
                stats.detail.insert(
                    format!("journal.{}.cursor", volume_id.short()),
                    state.cursor_usn.to_string(),
                );
                stats.detail.insert(
                    format!("journal.{}.next_usn", volume_id.short()),
                    state.next_usn.to_string(),
                );
            }
            if !guard.journals.is_empty() {
                stats.detail.insert(
                    "journals_healthy".into(),
                    guard.journals.all_healthy().to_string(),
                );
                for (volume_id, status) in guard.journals.degraded() {
                    stats
                        .warnings
                        .push(format!("{}: {}", volume_id.short(), status.as_str()));
                }
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

/// The pre-v0.2 candidate path: scan every record in order.
///
/// This is kept deliberately, and deliberately unchanged, as the reference
/// implementation the accelerated path is tested against. If the two ever
/// disagree, the accelerator is wrong.
#[must_use]
pub fn collect_candidates_linear(
    store: &FileStore,
    query: &SearchQuery,
    cap: usize,
) -> Vec<LocalEntity> {
    let prepared = PreparedQuery::new(query);
    let mut out = Vec::new();
    for (offset, record) in store.records().iter().enumerate() {
        if out.len() >= cap {
            break;
        }
        if record.is_deleted() {
            continue;
        }
        let Ok(id) = RecordId::try_from(offset) else {
            break;
        };
        if !prepared.accepts_record(record) || !cheap_filters_ok(store, id, record, query) {
            continue;
        }
        if !prepared.text_matches(store, id) {
            continue;
        }
        if let Some(entity) = store.entity(id) {
            out.push(entity);
        }
    }
    out
}

/// The accelerated candidate path, with the plan that produced it.
#[must_use]
pub fn collect_candidates_with_plan(
    store: &FileStore,
    accelerators: &SearchAccelerators,
    query: &SearchQuery,
    cap: usize,
) -> (Vec<LocalEntity>, QueryPlanInfo) {
    let planned = planner::plan(store, accelerators, query);
    let prepared = PreparedQuery::new(query);
    let mut out = Vec::new();
    let mut after_filters = 0usize;
    let mut verified = 0usize;
    let mut truncated = false;

    let consider = |id: RecordId,
                    out: &mut Vec<LocalEntity>,
                    after_filters: &mut usize,
                    verified: &mut usize,
                    truncated: &mut bool| {
        let Some(record) = store.records().get(id as usize).copied() else {
            return;
        };
        if !prepared.accepts_record(&record) || !cheap_filters_ok(store, id, &record, query) {
            return;
        }
        *after_filters += 1;
        if !prepared.text_matches(store, id) {
            return;
        }
        *verified += 1;
        if let Some(entity) = store.entity(id) {
            out.push(entity);
        }
        if out.len() >= cap {
            *truncated = true;
        }
    };

    match planned.source {
        CandidateSource::Ids => {
            for id in &planned.ids {
                if out.len() >= cap {
                    truncated = true;
                    break;
                }
                consider(
                    *id,
                    &mut out,
                    &mut after_filters,
                    &mut verified,
                    &mut truncated,
                );
            }
        }
        CandidateSource::LinearScan => {
            for offset in 0..store.len() {
                if out.len() >= cap {
                    truncated = true;
                    break;
                }
                if let Ok(id) = RecordId::try_from(offset) {
                    consider(
                        id,
                        &mut out,
                        &mut after_filters,
                        &mut verified,
                        &mut truncated,
                    );
                }
            }
        }
    }

    let mut info = planned.info;
    info.after_filters = after_filters;
    info.verified = verified;
    info.truncated = truncated;
    (out, info)
}

/// The parts of a query that every candidate must satisfy, computed once.
#[derive(Debug)]
struct PreparedQuery<'a> {
    wants_files: bool,
    wants_directories: bool,
    tokens: Vec<String>,
    path_filters: Vec<&'a str>,
}

impl<'a> PreparedQuery<'a> {
    fn new(query: &'a SearchQuery) -> Self {
        Self {
            wants_files: query.accepts_type(EntityType::File),
            wants_directories: query.accepts_type(EntityType::Directory),
            tokens: query
                .text
                .as_deref()
                .unwrap_or("")
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            path_filters: query
                .filters
                .iter()
                .filter_map(|filter| match filter {
                    Filter::Path(needle) => Some(needle.as_str()),
                    _ => None,
                })
                .collect(),
        }
    }

    fn accepts_record(&self, record: &FileRecord) -> bool {
        if record.is_directory() {
            self.wants_directories
        } else {
            self.wants_files
        }
    }

    /// Verify one candidate against the real strings in the store.
    ///
    /// This is the authority: an accelerator only proposes, this decides.
    fn text_matches(&self, store: &FileStore, id: RecordId) -> bool {
        if self.tokens.is_empty() && self.path_filters.is_empty() {
            return true;
        }
        let name = store.name(id);
        if !self.tokens.is_empty() {
            let all_in_name = self
                .tokens
                .iter()
                .all(|token| contains_ignore_case(name, token));
            if !all_in_name {
                // A path hit is worth rebuilding the string, but only when the
                // name matched at least one token: otherwise a one million
                // entry store would build a million paths.
                let any_in_name = self
                    .tokens
                    .iter()
                    .any(|token| contains_ignore_case(name, token));
                if !any_in_name {
                    return false;
                }
                let path = store.path_of(id);
                if !self
                    .tokens
                    .iter()
                    .all(|token| contains_ignore_case(&path, token))
                {
                    return false;
                }
            }
        }
        if !self.path_filters.is_empty() {
            let path = store.path_of(id);
            if !self
                .path_filters
                .iter()
                .all(|needle| contains_ignore_case(&path, needle))
            {
                return false;
            }
        }
        true
    }
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
            // Name predicates are decided here rather than left to the engine,
            // so the accelerated and linear candidate paths apply exactly the
            // same predicate set and can be compared directly.
            Filter::Name(needle) => contains_ignore_case(store.name(index), needle),
            Filter::NamePrefix(prefix) => starts_with_ignore_case(store.name(index), prefix),
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

/// The plan a query would use, without running it.
#[must_use]
pub fn explain_plan(
    store: &FileStore,
    accelerators: &SearchAccelerators,
    query: &SearchQuery,
) -> QueryPlanInfo {
    planner::plan(store, accelerators, query).info
}

/// Whether a name would be answered by the trigram plan.
#[must_use]
pub fn plan_kind_for(
    store: &FileStore,
    accelerators: &SearchAccelerators,
    token: &str,
) -> SearchPlan {
    let query = SearchQuery::plain(token);
    planner::plan(store, accelerators, &query).info.plan
}

/// The normalised key the accelerators use for a name.
#[must_use]
pub fn normalized_key(name: &str) -> Vec<u8> {
    normalise_key(name)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::TempTree;

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
        // The cache label must be unique per test: tests run in parallel and
        // share one cache directory.
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
        assert_eq!(provider.explain(&query).plan, SearchPlan::ExtensionLookup);
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
        // The accelerators cannot answer a path predicate honestly.
        assert_eq!(provider.explain(&query).plan, SearchPlan::LinearFallback);
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

        {
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
        }

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
    fn stats_expose_the_backend_accelerators_and_entry_counts() {
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
        assert!(stats.detail.contains_key("extensions_indexed"));
        assert!(stats.detail.contains_key("trigrams_indexed"));
        assert!(stats.detail.contains_key("accelerator_memory_bytes"));
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

    #[test]
    fn mutations_are_applied_and_indexed() {
        use crate::mutation::IndexMutation;

        let tree = TempTree::new("provider-mutations").unwrap();
        tree.write("seed.txt", b"x").unwrap();
        let provider = provider_for(&tree);
        provider.rebuild().unwrap();

        let root = {
            let guard = provider.runtime.read().unwrap();
            guard.index.store.root_index('C').unwrap()
        };
        let report = provider.apply_mutations(&[IndexMutation::Create {
            parent_file_id: 5,
            file_id: 900_001,
            name: "freshly-created.pdf".into(),
            is_directory: false,
            drive: 'C',
            size: 10,
            modified_ms: 1,
        }]);
        assert_eq!(report.created, 1);
        let _ = root;

        // The new record is searchable immediately, through the accelerator.
        let hits = provider
            .collect(&SearchQuery::plain("freshly-created"))
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name(), "freshly-created.pdf");
        assert_eq!(
            provider.explain(&SearchQuery::plain("freshly")).plan,
            SearchPlan::TrigramLookup
        );

        // And a delete hides it again.
        provider.apply_mutations(&[IndexMutation::Delete { file_id: 900_001 }]);
        assert!(provider
            .collect(&SearchQuery::plain("freshly-created"))
            .unwrap()
            .is_empty());
    }

    // -----------------------------------------------------------------------
    // Differential testing: the accelerated path must return exactly what the
    // reference linear scan returns. This is the contract that makes the
    // optimisation safe, and it is enforced over a randomised corpus rather
    // than a handful of hand-written cases.
    // -----------------------------------------------------------------------

    /// A tiny deterministic generator, so a failure can be reproduced.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 11
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }
    }

    /// A store of plausible names, plus a few contrived edge cases.
    fn synthetic_store(entries: usize, seed: u64) -> FileStore {
        const STEMS: [&str; 12] = [
            "report", "notes", "invoice", "design", "index", "search", "config", "session",
            "project", "archive", "visual", "studio",
        ];
        const EXTS: [&str; 8] = ["rs", "toml", "md", "json", "pdf", "txt", "png", "ts"];

        let mut rng = Lcg(seed);
        let mut store = FileStore::new();
        let root = store.push_root('C');
        let mut directories: Vec<u32> = vec![root];
        let mut created = 1usize;

        while created < entries {
            let parent = directories[rng.below(directories.len())];
            let stem = STEMS[rng.below(STEMS.len())];
            let number = rng.below(100_000);
            if rng.below(7) == 0 {
                let index = store.push_entry(
                    parent,
                    &format!("{stem}-{number}"),
                    'C',
                    true,
                    0,
                    1_700_000_000_000 + number as i64,
                    0,
                    0,
                );
                directories.push(index);
            } else {
                let extension = EXTS[rng.below(EXTS.len())];
                store.push_entry(
                    parent,
                    &format!("{stem}-{number}.{extension}"),
                    'C',
                    false,
                    number as u64,
                    1_700_000_000_000 + number as i64,
                    0,
                    0,
                );
            }
            created += 1;
        }

        // Edge cases that have historically broken substring matching.
        catalog_edge_cases(&mut store, root);
        store
    }

    fn catalog_edge_cases(store: &mut FileStore, root: u32) {
        for name in [
            "年度报告.pdf",
            "README.MD",
            "a.txt",
            "ab.txt",
            "abc.txt",
            "Visual Studio Code.exe",
            "code.exe",
            "xcode.exe",
            "with space and-dash_under.txt",
        ] {
            store.push_entry(root, name, 'C', false, 1, 0, 0, 0);
        }
    }

    fn entity_names(entities: &[LocalEntity]) -> Vec<String> {
        entities
            .iter()
            .map(|entity| format!("{}|{}", entity.entity_type(), entity.name()))
            .collect()
    }

    #[test]
    fn accelerated_search_equals_linear_search_over_a_random_corpus() {
        let store = synthetic_store(2_500, 0x5EED_1234_ABCD_9876);
        let accelerators = SearchAccelerators::build(&store);

        let queries = [
            "report",
            "report-1",
            "studio",
            "vis",
            "visual",
            "code",
            "xcode",
            "code.exe",
            "readme",
            "README",
            "年度",
            "年度报告",
            "a",
            "ab",
            "abc",
            "zzz",
            "no-such-name",
            "with space",
            "space and",
            "dash_under",
            "notes toml",
            "report pdf",
            "type:file ext:rs",
            "type:file ext:pdf report",
            "ext:md readme",
            "type:file ext:ts studio",
            "type:directory studio",
            "prefix:visual",
            "prefix:code",
            "prefix:xyz",
            "type:file prefix:read",
            "studio sort:name-asc",
        ];

        for dsl in queries {
            let query = query_dsl::parse(dsl).query;
            let reference = collect_candidates_linear(&store, &query, CANDIDATE_CAP);
            let (accelerated, info) =
                collect_candidates_with_plan(&store, &accelerators, &query, CANDIDATE_CAP);

            assert_eq!(
                entity_names(&accelerated),
                entity_names(&reference),
                "query `{dsl}` diverged (plan {}, sources {}): accelerated {} vs linear {}",
                info.plan,
                info.describe_sources(),
                accelerated.len(),
                reference.len()
            );
        }
    }

    #[test]
    fn accelerated_search_equals_linear_search_when_the_cap_truncates() {
        let store = synthetic_store(2_000, 42);
        let accelerators = SearchAccelerators::build(&store);
        // A tiny cap forces both paths to stop early; they must stop at the
        // same place, which is only true if the candidate order is by record id.
        let query = SearchQuery::plain("report");
        let reference = collect_candidates_linear(&store, &query, 5);
        let (accelerated, info) = collect_candidates_with_plan(&store, &accelerators, &query, 5);
        assert_eq!(entity_names(&accelerated), entity_names(&reference));
        assert!(info.truncated);
        assert_eq!(accelerated.len(), 5);
    }

    #[test]
    fn accelerated_search_equals_linear_search_after_mutations() {
        use crate::mutation::IndexMutation;

        let mut store = synthetic_store(1_200, 7);
        let mut accelerators = SearchAccelerators::build(&store);
        let root = store.root_index('C').unwrap();

        // Create, then rename, then delete, updating both sides the same way.
        let mutations = vec![
            IndexMutation::Create {
                parent_file_id: 5,
                file_id: 5_000_001,
                name: "brand-new-report.pdf".into(),
                is_directory: false,
                drive: 'C',
                size: 1,
                modified_ms: 1,
            },
            IndexMutation::Create {
                parent_file_id: 5,
                file_id: 5_000_002,
                name: "temporary.txt".into(),
                is_directory: false,
                drive: 'C',
                size: 1,
                modified_ms: 1,
            },
        ];
        store.apply_batch(&mutations);
        for mutation in &mutations {
            if let IndexMutation::Create { file_id, .. } = mutation {
                if let Some(id) = store.find_by_file_id(*file_id) {
                    accelerators.insert(&store, id);
                }
            }
        }

        let rename = vec![IndexMutation::Rename {
            file_id: 5_000_001,
            new_name: "renamed-archive.pdf".into(),
        }];
        store.apply_batch(&rename);
        accelerators.insert(&store, store.find_by_file_id(5_000_001).unwrap());

        let delete = vec![IndexMutation::Delete { file_id: 5_000_002 }];
        store.apply_batch(&delete);
        accelerators.note_mutations(1);

        let _ = root;
        for dsl in [
            "report",
            "archive",
            "renamed",
            "brand-new",
            "temporary",
            "type:file ext:pdf",
            "type:file ext:txt",
            "archive pdf",
        ] {
            let query = query_dsl::parse(dsl).query;
            let reference = collect_candidates_linear(&store, &query, CANDIDATE_CAP);
            let (accelerated, info) =
                collect_candidates_with_plan(&store, &accelerators, &query, CANDIDATE_CAP);
            assert_eq!(
                entity_names(&accelerated),
                entity_names(&reference),
                "`{dsl}` diverged after mutations (plan {})",
                info.plan
            );
        }
    }

    #[test]
    fn a_selective_query_plans_through_the_trigram_index() {
        let store = synthetic_store(20_000, 99);
        let accelerators = SearchAccelerators::build(&store);

        let query = SearchQuery::plain("studio-98765");
        let info = explain_plan(&store, &accelerators, &query);
        assert_eq!(info.plan, SearchPlan::TrigramLookup);
        assert!(
            info.initial_candidates < 200,
            "expected a selective candidate set, got {}",
            info.initial_candidates
        );

        let (accelerated, _) =
            collect_candidates_with_plan(&store, &accelerators, &query, CANDIDATE_CAP);
        let reference = collect_candidates_linear(&store, &query, CANDIDATE_CAP);
        assert_eq!(entity_names(&accelerated), entity_names(&reference));
    }

    #[test]
    fn an_extension_query_never_scans_the_index() {
        let store = synthetic_store(5_000, 3);
        let accelerators = SearchAccelerators::build(&store);
        let query = SearchQuery::plain("").with_filter(Filter::Extension("pdf".into()));
        let info = explain_plan(&store, &accelerators, &query);
        assert_eq!(info.plan, SearchPlan::ExtensionLookup);
        assert!(info.plan.is_exact());
        assert!(info.initial_candidates < store.len());
    }

    #[test]
    fn compacting_the_accelerators_preserves_results() {
        let store = synthetic_store(500, 11);
        let mut accelerators = SearchAccelerators::build(&store);
        // Pretend a lot of churn happened.
        accelerators.note_mutations(50_000);
        assert!(accelerators.needs_compaction(store.len()));

        accelerators = SearchAccelerators::build(&store);
        let query = SearchQuery::plain("report");
        let (accelerated, _) =
            collect_candidates_with_plan(&store, &accelerators, &query, CANDIDATE_CAP);
        let reference = collect_candidates_linear(&store, &query, CANDIDATE_CAP);
        assert_eq!(entity_names(&accelerated), entity_names(&reference));
    }
}
