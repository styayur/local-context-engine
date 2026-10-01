//! Uniform index mutations.
//!
//! Parsing a change journal and applying a change to the index are different
//! jobs, and mixing them is how a parser bug turns into a corrupted index. The
//! journal side produces [`IndexMutation`] values; the store side applies a
//! batch of them under one short write lock.
//!
//! ```text
//!   USN_RECORD_V2  --parse-->  IndexMutation  --apply-->  FileStore
//!        (mft)                     (this)                 (store.rs)
//! ```

use serde::{Deserialize, Serialize};

/// One change to apply to the index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mutation", rename_all = "kebab-case")]
pub enum IndexMutation {
    /// A file or directory appeared.
    Create {
        /// File reference number of the containing directory.
        parent_file_id: u64,
        /// File reference number of the new entry.
        file_id: u64,
        /// Entry name.
        name: String,
        /// Whether the entry is a directory.
        is_directory: bool,
        /// Volume the entry lives on, as a drive letter.
        drive: char,
        /// Size in bytes, when the source knows it.
        size: u64,
        /// Last write time in Unix milliseconds.
        modified_ms: i64,
    },
    /// A file or directory disappeared.
    Delete {
        /// File reference number of the removed entry.
        file_id: u64,
    },
    /// An entry was renamed in place.
    Rename {
        /// File reference number of the renamed entry.
        file_id: u64,
        /// The new name.
        new_name: String,
    },
    /// Contents or timestamps changed.
    MetadataChanged {
        /// File reference number of the entry.
        file_id: u64,
        /// New size in bytes, when the source knows it.
        ///
        /// `USN_RECORD_V2` does not carry a size, so the journal reader leaves
        /// this `None` rather than reporting a zero that would clobber a real
        /// size in the store.
        size: Option<u64>,
        /// New last write time in Unix milliseconds.
        modified_ms: i64,
    },
}

impl IndexMutation {
    /// The file reference number this mutation concerns.
    #[must_use]
    pub const fn file_id(&self) -> u64 {
        match self {
            IndexMutation::Create { file_id, .. }
            | IndexMutation::Delete { file_id }
            | IndexMutation::Rename { file_id, .. }
            | IndexMutation::MetadataChanged { file_id, .. } => *file_id,
        }
    }

    /// A short label for logs and `--explain`.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            IndexMutation::Create { .. } => "create",
            IndexMutation::Delete { .. } => "delete",
            IndexMutation::Rename { .. } => "rename",
            IndexMutation::MetadataChanged { .. } => "metadata",
        }
    }
}

/// What a batch of mutations did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    /// Entries added.
    pub created: usize,
    /// Entries tombstoned.
    pub deleted: usize,
    /// Entries given a new name.
    pub renamed: usize,
    /// Entries whose metadata was updated.
    pub metadata: usize,
    /// Mutations that could not be applied, usually because the parent is not
    /// in the index. A skipped mutation is reported, never guessed at.
    pub skipped: usize,
}

impl ApplyReport {
    /// Total mutations that changed something.
    #[must_use]
    pub const fn applied(&self) -> usize {
        self.created + self.deleted + self.renamed + self.metadata
    }

    /// Whether nothing was applied.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.applied() == 0 && self.skipped == 0
    }

    /// Accumulate another report.
    pub fn merge(&mut self, other: &ApplyReport) {
        self.created += other.created;
        self.deleted += other.deleted;
        self.renamed += other.renamed;
        self.metadata += other.metadata;
        self.skipped += other.skipped;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mutation_reports_its_file_id() {
        assert_eq!(IndexMutation::Delete { file_id: 7 }.file_id(), 7);
        assert_eq!(
            IndexMutation::Rename {
                file_id: 9,
                new_name: "b".into()
            }
            .file_id(),
            9
        );
        assert_eq!(
            IndexMutation::Create {
                parent_file_id: 1,
                file_id: 2,
                name: "a".into(),
                is_directory: false,
                drive: 'C',
                size: 0,
                modified_ms: 0,
            }
            .file_id(),
            2
        );
    }

    #[test]
    fn kinds_are_stable_wire_values() {
        let mutations = [
            IndexMutation::Delete { file_id: 1 },
            IndexMutation::Rename {
                file_id: 1,
                new_name: "n".into(),
            },
            IndexMutation::MetadataChanged {
                file_id: 1,
                size: Some(2),
                modified_ms: 3,
            },
        ];
        for mutation in &mutations {
            let json = serde_json::to_string(mutation).unwrap();
            assert!(json.contains(mutation.kind()), "{json}");
        }
    }

    #[test]
    fn a_report_totals_its_parts() {
        let report = ApplyReport {
            created: 1,
            deleted: 2,
            renamed: 3,
            metadata: 4,
            skipped: 5,
        };
        assert_eq!(report.applied(), 10);
        assert!(!report.is_empty());

        let mut merged = ApplyReport::default();
        merged.merge(&report);
        assert_eq!(merged, report);
    }

    #[test]
    fn an_empty_report_is_empty() {
        assert!(ApplyReport::default().is_empty());
    }

    #[test]
    fn mutations_round_trip_through_json() {
        let mutation = IndexMutation::Create {
            parent_file_id: 5,
            file_id: 6,
            name: "report.pdf".into(),
            is_directory: false,
            drive: 'D',
            size: 1_024,
            modified_ms: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&mutation).unwrap();
        let restored: IndexMutation = serde_json::from_str(&json).unwrap();
        assert_eq!(mutation, restored);
    }
}
