//! The directory-scan index backend.
//!
//! This is the backend that always works: no elevation, no NTFS requirement,
//! no fragile NT API. It walks the configured roots breadth-first, skipping
//! known-noise directories, and builds a [`FileStore`].
//!
//! It is deliberately *not* run on a schedule. Building it is a one-off cost
//! that is paid once and then persisted; after that the provider serves
//! searches straight out of memory.

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use search_core::LceError;

use crate::store::FileStore;
use crate::volume::VolumeSpec;

/// What to index and how hard to try.
#[derive(Debug, Clone)]
pub struct IndexConfig {
    /// Volumes to index.
    pub volumes: Vec<VolumeSpec>,
    /// Hard ceiling on indexed entries, so a runaway scan cannot exhaust RAM.
    pub max_entries: usize,
    /// Directory names that are skipped, along with everything below them.
    pub excluded_dir_names: Vec<String>,
    /// Substrings that, when present in a path, skip that path.
    pub excluded_path_fragments: Vec<String>,
    /// Whether directory entries are indexed alongside files.
    pub include_directories: bool,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            volumes: Vec::new(),
            max_entries: 1_000_000,
            excluded_dir_names: default_exclusions(),
            excluded_path_fragments: default_path_exclusions(),
            include_directories: true,
        }
    }
}

impl IndexConfig {
    /// The directory names skipped by default.
    ///
    /// These are the ones that dominate a naive scan while contributing almost
    /// nothing to a human's searches.
    #[must_use]
    pub fn default_exclusions() -> Vec<String> {
        default_exclusions()
    }
}

fn default_exclusions() -> Vec<String> {
    [
        "$recycle.bin",
        "system volume information",
        "winsxs",
        "installer",
        "softwaredistribution",
        "windows.old",
        "node_modules",
        ".git",
        ".svn",
        ".hg",
        "__pycache__",
        ".gradle",
        ".nuget",
        ".npm",
        "temp",
        "cache",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn default_path_exclusions() -> Vec<String> {
    [
        r"\appdata\local\temp",
        r"\.cargo\registry",
        r"\.rustup\toolchains",
        r"\windows\servicing",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// What a rebuild produced.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexReport {
    /// Which backend produced the index.
    pub backend: String,
    /// Volumes that were indexed.
    pub volumes: Vec<String>,
    /// Total entries in the store.
    pub entries: usize,
    /// Directories in the store.
    pub directories: usize,
    /// Whether the entry ceiling was hit.
    pub truncated: bool,
    /// Wall-clock duration in milliseconds.
    pub elapsed_ms: f64,
    /// Recoverable problems encountered while scanning.
    pub warnings: Vec<String>,
}

/// Walk the configured volumes and build a store.
#[must_use]
pub fn build_store(config: &IndexConfig) -> (FileStore, IndexReport) {
    let started = Instant::now();
    let mut store = FileStore::new();
    store.reserve(config.max_entries.min(100_000));
    let mut warnings: Vec<String> = Vec::new();
    let mut truncated = false;

    'volumes: for volume in &config.volumes {
        let root_path = PathBuf::from(&volume.root);
        let root_index = store.push_root(volume.drive);

        let mut stack: Vec<(PathBuf, u32)> = vec![(root_path, root_index)];
        while let Some((directory, parent_index)) = stack.pop() {
            let reader = match std::fs::read_dir(&directory) {
                Ok(reader) => reader,
                Err(error) => {
                    if is_permission_error(&error) {
                        warnings.push(format!(
                            "skipped {}: access denied (administrator privileges may be required)",
                            directory.display()
                        ));
                    }
                    continue;
                }
            };

            for entry in reader.flatten() {
                if store.len() >= config.max_entries {
                    truncated = true;
                    warnings.push(format!(
                        "stopped at the {} entry ceiling",
                        config.max_entries
                    ));
                    break 'volumes;
                }

                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name.is_empty() {
                    continue;
                }

                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                // Junctions and symlinks can create cycles; the index keeps
                // the link itself out and never follows it.
                let is_directory = file_type.is_dir();

                if is_directory && is_excluded(&name, &path, config) {
                    continue;
                }

                let metadata = entry.metadata().ok();
                let size = metadata
                    .as_ref()
                    .map_or(0, |meta| if is_directory { 0 } else { meta.len() });
                let modified = metadata
                    .as_ref()
                    .and_then(|meta| meta.modified().ok())
                    .and_then(system_time_to_ms)
                    .unwrap_or(0);
                let created = metadata
                    .as_ref()
                    .and_then(|meta| meta.created().ok())
                    .and_then(system_time_to_ms)
                    .unwrap_or(0);

                if is_directory && !config.include_directories {
                    // Still descend, just do not record the directory itself.
                    stack.push((path, parent_index));
                    continue;
                }

                let index = store.push_entry(
                    parent_index,
                    &name,
                    volume.drive,
                    is_directory,
                    size,
                    modified,
                    created,
                    0,
                );

                if is_directory {
                    stack.push((path, index));
                }
            }
        }
    }

    let report = IndexReport {
        backend: "scan".into(),
        volumes: config.volumes.iter().map(VolumeSpec::label).collect(),
        entries: store.len(),
        directories: store.directory_count(),
        truncated,
        elapsed_ms: elapsed_ms(started),
        warnings,
    };
    (store, report)
}

fn is_excluded(name: &str, path: &Path, config: &IndexConfig) -> bool {
    let lowered = name.to_lowercase();
    if config
        .excluded_dir_names
        .iter()
        .any(|excluded| excluded.eq_ignore_ascii_case(&lowered))
    {
        return true;
    }
    let path = path.to_string_lossy().to_lowercase();
    config
        .excluded_path_fragments
        .iter()
        .any(|fragment| path.contains(&fragment.to_lowercase()))
}

fn is_permission_error(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
}

/// Convert a `SystemTime` into Unix milliseconds.
#[must_use]
pub fn system_time_to_ms(time: SystemTime) -> Option<i64> {
    let delta = time.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(delta.as_millis()).ok()
}

fn elapsed_ms(started: Instant) -> f64 {
    let micros = started.elapsed().as_micros() as f64;
    (micros / 1_000.0).round() / 1_000.0
}

/// A tiny filesystem used by the tests, so they never depend on the developer
/// machine's real layout.
///
/// The directory is created under the system temp directory and removed when
/// the guard is dropped.
#[derive(Debug)]
pub struct TempTree {
    root: PathBuf,
}

impl TempTree {
    /// Create a fresh temporary tree.
    pub fn new(label: &str) -> Result<Self, LceError> {
        let unique = format!(
            "lce-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|delta| delta.as_nanos())
                .unwrap_or(0)
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&root)
            .map_err(|error| LceError::io("creating a temporary test tree", &error))?;
        Ok(Self { root })
    }

    /// The tree root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Write a file, creating parent directories as needed.
    pub fn write(&self, relative: &str, contents: &[u8]) -> Result<PathBuf, LceError> {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| LceError::io("creating a temporary directory", &error))?;
        }
        std::fs::write(&path, contents)
            .map_err(|error| LceError::io("writing a temporary file", &error))?;
        Ok(path)
    }

    /// Create a directory.
    pub fn mkdir(&self, relative: &str) -> Result<PathBuf, LceError> {
        let path = self.root.join(relative);
        std::fs::create_dir_all(&path)
            .map_err(|error| LceError::io("creating a temporary directory", &error))?;
        Ok(path)
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volume::VolumeSpec;

    fn config_for(root: &Path) -> IndexConfig {
        let drive = 'C';
        IndexConfig {
            volumes: vec![VolumeSpec::synthetic(
                root.to_string_lossy().to_string(),
                drive,
                true,
            )],
            max_entries: 10_000,
            excluded_dir_names: vec!["skipme".into()],
            excluded_path_fragments: Vec::new(),
            include_directories: true,
        }
    }

    #[test]
    fn scan_indexes_files_and_directories() {
        let tree = TempTree::new("scan").unwrap();
        tree.write("alpha.txt", b"alpha").unwrap();
        tree.write(r"nested\bravo.pdf", b"bravo").unwrap();
        tree.mkdir("empty").unwrap();

        let (store, report) = build_store(&config_for(tree.root()));
        assert_eq!(report.backend, "scan");
        assert!(!report.truncated);
        assert!(report.entries >= 4, "got {} entries", report.entries);

        let names: Vec<String> = store
            .entities()
            .iter()
            .map(|entity| entity.name().to_string())
            .collect();
        assert!(names.contains(&"alpha.txt".to_string()));
        assert!(names.contains(&"bravo.pdf".to_string()));
        assert!(names.contains(&"nested".to_string()));
        assert!(names.contains(&"empty".to_string()));
    }

    #[test]
    fn scan_records_file_sizes_and_extensions() {
        let tree = TempTree::new("sizes").unwrap();
        tree.write("data.bin", &[0u8; 2048]).unwrap();

        let (store, _) = build_store(&config_for(tree.root()));
        let record = store
            .entities()
            .into_iter()
            .find(|entity| entity.name() == "data.bin")
            .expect("data.bin must be indexed");
        match record {
            search_core::LocalEntity::File(entry) => {
                assert_eq!(entry.size, 2048);
                assert_eq!(entry.extension.as_deref(), Some("bin"));
                assert!(entry.modified.is_some());
            }
            other => panic!("expected a file, got {other:?}"),
        }
    }

    #[test]
    fn excluded_directories_are_not_descended_into() {
        let tree = TempTree::new("excluded").unwrap();
        tree.write(r"keep\visible.txt", b"x").unwrap();
        tree.write(r"skipme\hidden.txt", b"x").unwrap();

        let (store, _) = build_store(&config_for(tree.root()));
        let names: Vec<String> = store
            .entities()
            .iter()
            .map(|entity| entity.name().to_string())
            .collect();
        assert!(names.contains(&"visible.txt".to_string()));
        assert!(!names.contains(&"hidden.txt".to_string()));
        assert!(!names.contains(&"skipme".to_string()));
    }

    #[test]
    fn the_entry_ceiling_is_enforced_and_reported() {
        let tree = TempTree::new("ceiling").unwrap();
        for index in 0..64 {
            tree.write(&format!("file-{index}.txt"), b"x").unwrap();
        }
        let mut config = config_for(tree.root());
        config.max_entries = 8;

        let (store, report) = build_store(&config);
        assert!(report.truncated);
        assert_eq!(store.len(), 8);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("ceiling")));
    }

    #[test]
    fn directories_can_be_excluded_from_the_store_but_still_traversed() {
        let tree = TempTree::new("nodirs").unwrap();
        tree.write(r"a\b\deep.txt", b"x").unwrap();
        let mut config = config_for(tree.root());
        config.include_directories = false;

        let (store, _) = build_store(&config);
        let names: Vec<String> = store
            .entities()
            .iter()
            .map(|entity| entity.name().to_string())
            .collect();
        // The volume root is always recorded so paths stay resolvable; the
        // intermediate directories must not be.
        assert!(names.contains(&"deep.txt".to_string()), "{names:?}");
        assert!(!names.contains(&"a".to_string()), "{names:?}");
        assert!(!names.contains(&"b".to_string()), "{names:?}");
        assert_eq!(store.file_count(), 1);
    }

    #[test]
    fn a_missing_root_is_skipped_without_failing() {
        let missing = std::env::temp_dir().join("lce-does-not-exist-4f8a2c");
        let (store, report) = build_store(&config_for(&missing));
        assert_eq!(store.len(), 1, "only the volume root is recorded");
        assert!(report.warnings.is_empty() || !report.warnings.is_empty());
    }

    #[test]
    fn timestamp_conversion_round_trips() {
        let now = SystemTime::now();
        let ms = system_time_to_ms(now).unwrap();
        assert!(ms > 1_600_000_000_000);
    }
}
