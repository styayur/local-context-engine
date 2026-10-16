//! Index cache persistence.
//!
//! The whole point of an index is that it survives a restart. The store is
//! written with MessagePack, which is compact and fast, and every write goes
//! through a temporary file plus rename so an interrupted save can never leave
//! a half-written cache behind.
//!
//! ## Schema history
//!
//! | version | contents                                                     |
//! |---------|--------------------------------------------------------------|
//! | 1       | store + a single optional `JournalState` (v0.1)              |
//! | 2       | store + a per-volume `JournalRegistry`                       |
//! | 3       | store + per-volume journals + search accelerators (v0.2)     |
//!
//! An older cache is **never** migrated in place. A version mismatch means
//! "discard and rebuild", because guessing at the meaning of a cursor written
//! by an older build is exactly how a silently stale index happens.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use search_core::{clock, IndexError, LceError};

use crate::accelerators::SearchAccelerators;
use crate::journal::JournalRegistry;
use crate::store::FileStore;

/// Bumped whenever the on-disk shape changes. See the table above.
pub const CACHE_FORMAT_VERSION: u32 = 3;

/// Everything that has to survive a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedIndex {
    /// Cache format version, see [`CACHE_FORMAT_VERSION`].
    pub version: u32,
    /// Which backend produced the store.
    pub backend: String,
    /// Volumes that are indexed.
    pub volumes: Vec<String>,
    /// When the cache was written, in Unix milliseconds.
    pub saved_at_ms: i64,
    /// Per-volume journal cursors.
    #[serde(default)]
    pub journals: JournalRegistry,
    /// The index itself.
    pub store: FileStore,
    /// Search accelerators, rebuilt automatically when absent.
    #[serde(default)]
    pub accelerators: Option<SearchAccelerators>,
}

impl PersistedIndex {
    /// Wrap a store for saving, building the accelerators if they are missing.
    #[must_use]
    pub fn new(
        backend: impl Into<String>,
        volumes: Vec<String>,
        journals: JournalRegistry,
        store: FileStore,
        accelerators: SearchAccelerators,
    ) -> Self {
        Self {
            version: CACHE_FORMAT_VERSION,
            backend: backend.into(),
            volumes,
            saved_at_ms: clock::now_ms(),
            journals,
            store,
            accelerators: Some(accelerators),
        }
    }

    /// Whether this cache matches the running build.
    ///
    /// The journal registry has its own schema, so both have to agree.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.version == CACHE_FORMAT_VERSION && self.journals.is_current()
    }

    /// How old the cache is, in milliseconds.
    #[must_use]
    pub fn age_ms(&self) -> i64 {
        (clock::now_ms() - self.saved_at_ms).max(0)
    }

    /// Consume the cache, returning its parts.
    ///
    /// Accelerators are rebuilt when the cache predates them, which keeps the
    /// format change invisible to callers.
    #[must_use]
    pub fn into_parts(self) -> (FileStore, SearchAccelerators, JournalRegistry) {
        let store = self.store;
        let accelerators = self
            .accelerators
            .unwrap_or_else(|| SearchAccelerators::build(&store));
        (store, accelerators, self.journals)
    }
}

/// The directory that holds the index cache and the usage file.
///
/// `%LOCALAPPDATA%\LocalContextEngine`, falling back to the system temp
/// directory when the environment variable is missing.
#[must_use]
pub fn cache_root() -> PathBuf {
    match std::env::var_os("LOCALAPPDATA") {
        Some(local) => PathBuf::from(local).join("LocalContextEngine"),
        None => std::env::temp_dir().join("LocalContextEngine"),
    }
}

/// The cache file for one backend and volume set.
#[must_use]
pub fn index_path(label: &str) -> PathBuf {
    cache_root().join(format!("{label}.lce-index"))
}

/// Save an index atomically.
pub fn save(path: &Path, index: &PersistedIndex) -> Result<(), LceError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| LceError::io("creating the index cache directory", &error))?;
    }
    // Named encoding, not positional: `VolumeJournalState` skips `Option`
    // fields when they are `None`, and a positional encoding would shift every
    // following field and corrupt the read.
    let encoded = rmp_serde::to_vec_named(index).map_err(|error| LceError::Io {
        action: "encoding the index cache".into(),
        detail: error.to_string(),
    })?;

    let temporary = path.with_extension("lce-index.tmp");
    std::fs::write(&temporary, &encoded)
        .map_err(|error| LceError::io("writing the index cache", &error))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| LceError::io("committing the index cache", &error))?;
    Ok(())
}

/// Load an index cache.
///
/// Returns `Ok(None)` when the file is missing, in an older format, or
/// unreadable — all of which mean "rebuild", not "fail".
pub fn load(path: &Path) -> Result<Option<PersistedIndex>, LceError> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "index cache could not be read");
            return Ok(None);
        }
    };
    let index: PersistedIndex = match rmp_serde::from_slice(&bytes) {
        Ok(index) => index,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "index cache is not in a readable format; it will be rebuilt"
            );
            return Ok(None);
        }
    };
    if !index.is_current() {
        tracing::info!(
            found = index.version,
            expected = CACHE_FORMAT_VERSION,
            journal_schema = index.journals.schema_version,
            "index cache format changed; rebuilding"
        );
        return Ok(None);
    }
    Ok(Some(index))
}

/// Delete the cache file for a label.
pub fn remove(label: &str) -> Result<(), LceError> {
    let path = index_path(label);
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|error| LceError::io("removing the index cache", &error))?;
    }
    Ok(())
}

/// Validate that a path is inside the cache directory before deleting it.
///
/// `remove` only ever deletes a file this crate wrote, but the check keeps the
/// "never delete something we did not create" invariant explicit.
#[must_use]
pub fn is_cache_path(path: &Path) -> bool {
    let Ok(root) = cache_root().canonicalize() else {
        return false;
    };
    path.canonicalize()
        .map(|candidate| candidate.starts_with(&root))
        .unwrap_or(false)
}

/// The error a caller sees for a cache that exists but cannot be used.
#[must_use]
pub fn unreadable(path: &Path) -> LceError {
    LceError::Index(IndexError::CacheUnreadable {
        path: path.to_string_lossy().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accelerators::SearchAccelerators;
    use crate::journal::JournalRegistry;
    use crate::store::FileStore;

    fn sample_store() -> FileStore {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        store.push_entry(root, "report.pdf", 'C', false, 2048, 5, 6, 0);
        store
    }

    fn unique_path(label: &str) -> PathBuf {
        index_path(&format!("{label}-{}", std::process::id()))
    }

    #[test]
    fn a_store_round_trips_through_the_cache() {
        let path = unique_path("roundtrip");
        let store = sample_store();
        let index = PersistedIndex::new(
            "scan",
            vec!["C:".into()],
            JournalRegistry::new(),
            store.clone(),
            SearchAccelerators::build(&store),
        );
        save(&path, &index).unwrap();

        let loaded = load(&path).unwrap().expect("cache must load");
        assert_eq!(loaded.backend, "scan");
        assert_eq!(loaded.store.len(), 2);
        assert_eq!(loaded.store.name(1), "report.pdf");
        assert!(loaded.is_current());

        let (store, accelerators, journals) = loaded.into_parts();
        assert_eq!(store.len(), 2);
        assert_eq!(accelerators.extensions().lookup("pdf"), &[1]);
        assert!(journals.is_empty());

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_missing_cache_is_not_an_error() {
        let path = index_path("test-definitely-missing-cache");
        assert!(load(&path).unwrap().is_none());
    }

    #[test]
    fn a_corrupt_cache_is_reported_as_none_rather_than_failing() {
        let path = unique_path("corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"this is not messagepack").unwrap();
        assert!(load(&path).unwrap().is_none());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_v01_cache_version_is_ignored_instead_of_migrated() {
        let path = unique_path("v1");
        let store = sample_store();
        let mut index = PersistedIndex::new(
            "scan",
            vec![],
            JournalRegistry::new(),
            store.clone(),
            SearchAccelerators::build(&store),
        );
        // A v0.1 cache carried a single journal, not a registry.
        index.version = 1;
        save(&path, &index).unwrap();
        assert!(load(&path).unwrap().is_none(), "v1 must not be trusted");

        let mut future = index.clone();
        future.version = CACHE_FORMAT_VERSION + 1;
        save(&path, &future).unwrap();
        assert!(load(&path).unwrap().is_none(), "v4 must not be trusted");
        assert!(!future.is_current());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_cache_with_an_old_journal_schema_is_ignored() {
        let path = unique_path("oldjournal");
        let store = sample_store();
        let mut journals = JournalRegistry::new();
        journals.schema_version = crate::journal::JOURNAL_SCHEMA_VERSION - 1;
        let index = PersistedIndex::new(
            "mft-usn",
            vec!["C:".into()],
            journals,
            store.clone(),
            SearchAccelerators::build(&store),
        );
        save(&path, &index).unwrap();
        assert!(load(&path).unwrap().is_none());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_cache_without_accelerators_rebuilds_them_on_load() {
        let path = unique_path("noaccel");
        let store = sample_store();
        let mut index = PersistedIndex::new(
            "scan",
            vec![],
            JournalRegistry::new(),
            store.clone(),
            SearchAccelerators::build(&store),
        );
        index.accelerators = None;
        save(&path, &index).unwrap();

        let loaded = load(&path).unwrap().unwrap();
        let (_, accelerators, _) = loaded.into_parts();
        assert_eq!(accelerators.extensions().lookup("pdf"), &[1]);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn per_volume_journals_survive_a_round_trip() {
        let path = unique_path("journals");
        let store = sample_store();
        let mut journals = JournalRegistry::new();
        let identity = crate::volume::identity_for_drive('C').unwrap_or_else(|| {
            crate::volume::VolumeIdentity::new(crate::volume::VolumeId::from_serial(1), 1, "NTFS")
        });
        journals.register(&identity);
        let id = identity.id.clone();
        journals.advance(&id, 4_242, 7);

        let index = PersistedIndex::new(
            "mft-usn",
            vec!["C:".into()],
            journals,
            store.clone(),
            SearchAccelerators::build(&store),
        );
        save(&path, &index).unwrap();
        let loaded = load(&path).unwrap().unwrap();
        assert_eq!(loaded.journals.get(&id).unwrap().cursor_usn, 4_242);
        assert_eq!(loaded.journals.get(&id).unwrap().entries, 7);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn the_cache_lives_under_the_cache_root() {
        let path = index_path("demo");
        assert!(path.starts_with(cache_root()));
        assert_eq!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("lce-index")
        );
    }

    #[test]
    fn saving_is_atomic_and_leaves_no_temporary_file() {
        let path = unique_path("atomic");
        let store = sample_store();
        let index = PersistedIndex::new(
            "scan",
            vec![],
            JournalRegistry::new(),
            store.clone(),
            SearchAccelerators::build(&store),
        );
        save(&path, &index).unwrap();
        assert!(path.exists());
        assert!(!path.with_extension("lce-index.tmp").exists());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_fresh_cache_reports_its_age() {
        let store = sample_store();
        let index = PersistedIndex::new(
            "scan",
            vec![],
            JournalRegistry::new(),
            store.clone(),
            SearchAccelerators::build(&store),
        );
        assert!(index.age_ms() < 5_000);
    }

    #[test]
    fn the_schema_version_is_three() {
        assert_eq!(CACHE_FORMAT_VERSION, 3);
    }
}
