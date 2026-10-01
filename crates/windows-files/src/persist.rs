//! Index cache persistence.
//!
//! The whole point of an index is that it survives a restart. The store is
//! written with MessagePack, which is compact and fast, and every write goes
//! through a temporary file plus rename so an interrupted save can never leave
//! a half-written cache behind.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use search_core::{clock, IndexError, LceError};

use crate::mft::JournalState;
use crate::store::FileStore;

/// Bumped whenever [`FileStore`]'s on-disk shape changes. A mismatch is not an
/// error: it simply means the cache is ignored and rebuilt.
pub const CACHE_FORMAT_VERSION: u32 = 1;

/// Everything that has to survive a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedIndex {
    /// Format version, see [`CACHE_FORMAT_VERSION`].
    pub version: u32,
    /// Which backend produced the store.
    pub backend: String,
    /// Volumes that are indexed.
    pub volumes: Vec<String>,
    /// When the cache was written, in Unix milliseconds.
    pub saved_at_ms: i64,
    /// Change journal position, for the MFT backend.
    pub journal: Option<JournalState>,
    /// The index itself.
    pub store: FileStore,
}

impl PersistedIndex {
    /// Wrap a store for saving.
    #[must_use]
    pub fn new(
        backend: impl Into<String>,
        volumes: Vec<String>,
        journal: Option<JournalState>,
        store: FileStore,
    ) -> Self {
        Self {
            version: CACHE_FORMAT_VERSION,
            backend: backend.into(),
            volumes,
            saved_at_ms: clock::now_ms(),
            journal,
            store,
        }
    }

    /// Whether this cache matches the running build.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.version == CACHE_FORMAT_VERSION
    }

    /// How old the cache is, in milliseconds.
    #[must_use]
    pub fn age_ms(&self) -> i64 {
        (clock::now_ms() - self.saved_at_ms).max(0)
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
    let encoded = rmp_serde::to_vec(index).map_err(|error| LceError::Io {
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
                "index cache is not in a readable format"
            );
            return Ok(None);
        }
    };
    if !index.is_current() {
        tracing::info!(
            found = index.version,
            expected = CACHE_FORMAT_VERSION,
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
    use crate::store::FileStore;

    fn sample_store() -> FileStore {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        store.push_entry(root, "report.pdf", 'C', false, 2048, 5, 6, 0);
        store
    }

    #[test]
    fn a_store_round_trips_through_the_cache() {
        let label = format!("test-roundtrip-{}", std::process::id());
        let path = index_path(&label);
        let index = PersistedIndex::new("scan", vec!["C:".into()], None, sample_store());
        save(&path, &index).unwrap();

        let loaded = load(&path).unwrap().expect("cache must load");
        assert_eq!(loaded.backend, "scan");
        assert_eq!(loaded.store.len(), 2);
        assert_eq!(loaded.store.name(1), "report.pdf");
        assert!(loaded.is_current());

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_missing_cache_is_not_an_error() {
        let path = index_path("test-definitely-missing-cache");
        assert!(load(&path).unwrap().is_none());
    }

    #[test]
    fn a_corrupt_cache_is_reported_as_none_rather_than_failing() {
        let label = format!("test-corrupt-{}", std::process::id());
        let path = index_path(&label);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"this is not messagepack").unwrap();
        assert!(load(&path).unwrap().is_none());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn an_old_format_version_is_ignored() {
        let label = format!("test-version-{}", std::process::id());
        let path = index_path(&label);
        let mut index = PersistedIndex::new("scan", vec![], None, sample_store());
        index.version = CACHE_FORMAT_VERSION + 1;
        save(&path, &index).unwrap();
        assert!(load(&path).unwrap().is_none());
        assert!(!index.is_current());
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
        let label = format!("test-atomic-{}", std::process::id());
        let path = index_path(&label);
        let index = PersistedIndex::new("scan", vec![], None, sample_store());
        save(&path, &index).unwrap();
        assert!(path.exists());
        assert!(!path.with_extension("lce-index.tmp").exists());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_fresh_cache_reports_its_age() {
        let index = PersistedIndex::new("scan", vec![], None, sample_store());
        assert!(index.age_ms() < 5_000);
    }

    #[test]
    fn journal_state_is_preserved() {
        let label = format!("test-journal-{}", std::process::id());
        let path = index_path(&label);
        let state = JournalState {
            journal_id: 99,
            next_usn: 4_242,
        };
        let index = PersistedIndex::new("mft-usn", vec!["C:".into()], Some(state), sample_store());
        save(&path, &index).unwrap();
        let loaded = load(&path).unwrap().unwrap();
        assert_eq!(loaded.journal, Some(state));
        std::fs::remove_file(&path).unwrap();
    }
}
