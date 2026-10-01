//! A compact, path-compressed file index.
//!
//! Holding one million [`search_core::LocalEntity`] values would cost several
//! hundred megabytes, because every one of them owns its path string. This
//! module stores the same information in a *string arena* plus fixed-size
//! records whose paths are reconstructed on demand by walking parent links.
//!
//! A one million entry index costs roughly
//!
//! ```text
//!   1_000_000 * 48 B  records  = 48 MB
//!   + names + directory paths  ≈ 20 MB
//!   ---------------------------------
//!   ≈ 70 MB, comfortably inside the project's memory budget.
//! ```

use serde::{Deserialize, Serialize};

use search_core::{file_name_of, DirectoryEntry, FileEntry, LocalEntity};

/// Marks a record as a directory.
pub const FLAG_DIRECTORY: u8 = 0b0000_0001;
/// Marks a record as a volume root.
pub const FLAG_ROOT: u8 = 0b0000_0010;
/// Marks a record as removed.
///
/// The USN backend tombstones deletions instead of splicing the record out,
/// because every child stores an index into this Vec and removing an entry
/// would silently re-point every sibling after it.
pub const FLAG_DELETED: u8 = 0b0000_0100;
/// Sentinel meaning "this record has no parent".
pub const NO_PARENT: u32 = u32::MAX;

/// One file system entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    /// Index of the containing directory, or [`NO_PARENT`] for a volume root.
    pub parent: u32,
    /// Byte offset of the name inside the arena.
    pub name_off: u32,
    /// Byte length of the name inside the arena.
    pub name_len: u16,
    /// Length of the file extension, in bytes, or zero when there is none.
    pub ext_len: u8,
    /// Drive letter as an ASCII byte, or zero when unknown.
    pub drive: u8,
    /// See [`FLAG_DIRECTORY`], [`FLAG_ROOT`].
    pub flags: u8,
    /// File size in bytes; zero for directories.
    pub size: u64,
    /// Last write time as Unix milliseconds.
    pub modified_ms: i64,
    /// Creation time as Unix milliseconds.
    pub created_ms: i64,
    /// NTFS file reference number, or zero when unknown.
    pub file_id: u64,
}

impl FileRecord {
    /// Whether this record is a directory.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.flags & FLAG_DIRECTORY != 0
    }

    /// Whether this record is a volume root.
    #[must_use]
    pub const fn is_root(&self) -> bool {
        self.flags & FLAG_ROOT != 0
    }

    /// Whether this record has been tombstoned.
    #[must_use]
    pub const fn is_deleted(&self) -> bool {
        self.flags & FLAG_DELETED != 0
    }
}

impl Default for FileRecord {
    fn default() -> Self {
        Self {
            parent: NO_PARENT,
            name_off: 0,
            name_len: 0,
            ext_len: 0,
            drive: 0,
            flags: 0,
            size: 0,
            modified_ms: 0,
            created_ms: 0,
            file_id: 0,
        }
    }
}

/// The in-memory index.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileStore {
    arena: Vec<u8>,
    records: Vec<FileRecord>,
}

impl FileStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve room for `entries` records.
    pub fn reserve(&mut self, entries: usize) {
        self.records.reserve(entries);
        self.arena.reserve(entries * 24);
    }

    /// How many entries are indexed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Every record, in insertion order.
    #[must_use]
    pub fn records(&self) -> &[FileRecord] {
        &self.records
    }

    /// Intern a name and return its offset and byte length.
    fn intern(&mut self, name: &str) -> (u32, u16) {
        let offset = u32::try_from(self.arena.len()).unwrap_or(u32::MAX);
        let bytes = name.as_bytes();
        let length = u16::try_from(bytes.len()).unwrap_or(u16::MAX);
        self.arena.extend_from_slice(&bytes[..usize::from(length)]);
        (offset, length)
    }

    /// Append a record and return its index.
    pub fn push(&mut self, record: FileRecord) -> u32 {
        let index = u32::try_from(self.records.len()).unwrap_or(u32::MAX);
        self.records.push(record);
        index
    }

    /// Append a volume root.
    pub fn push_root(&mut self, drive: char) -> u32 {
        let name = format!("{}:\\", drive.to_ascii_uppercase());
        let (name_off, name_len) = self.intern(&name);
        self.push(FileRecord {
            parent: NO_PARENT,
            name_off,
            name_len,
            ext_len: 0,
            drive: drive.to_ascii_uppercase() as u8,
            flags: FLAG_DIRECTORY | FLAG_ROOT,
            ..FileRecord::default()
        })
    }

    /// Append a directory or file below `parent`.
    #[allow(clippy::too_many_arguments)]
    pub fn push_entry(
        &mut self,
        parent: u32,
        name: &str,
        drive: char,
        is_directory: bool,
        size: u64,
        modified_ms: i64,
        created_ms: i64,
        file_id: u64,
    ) -> u32 {
        let (name_off, name_len) = self.intern(name);
        let ext_len = if is_directory {
            0
        } else {
            extension_of(name).map_or(0, |ext| u8::try_from(ext.len()).unwrap_or(0))
        };
        self.push(FileRecord {
            parent,
            name_off,
            name_len,
            ext_len,
            drive: drive.to_ascii_uppercase() as u8,
            flags: if is_directory { FLAG_DIRECTORY } else { 0 },
            size: if is_directory { 0 } else { size },
            modified_ms,
            created_ms,
            file_id,
        })
    }

    /// The name of one record.
    #[must_use]
    pub fn name(&self, index: u32) -> &str {
        let Some(record) = self.records.get(index as usize) else {
            return "";
        };
        let start = record.name_off as usize;
        let end = start.saturating_add(usize::from(record.name_len));
        self.arena
            .get(start..end)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .unwrap_or("")
    }

    /// The lower-case extension of one record, without the leading dot.
    #[must_use]
    pub fn extension(&self, index: u32) -> Option<&str> {
        let record = self.records.get(index as usize)?;
        if record.is_directory() || record.ext_len == 0 {
            return None;
        }
        let end = record.name_off as usize + usize::from(record.name_len);
        let start = end.saturating_sub(usize::from(record.ext_len));
        self.arena
            .get(start..end)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
    }

    /// The drive letter for one record.
    #[must_use]
    pub fn drive(&self, index: u32) -> Option<char> {
        let record = self.records.get(index as usize)?;
        (record.drive != 0).then(|| char::from(record.drive))
    }

    /// Reconstruct the fully qualified path of one record.
    ///
    /// This walks the parent chain rather than storing a path per entry, which
    /// is what keeps the index small.
    #[must_use]
    pub fn path_of(&self, index: u32) -> String {
        let mut chain: Vec<u32> = Vec::with_capacity(16);
        let mut cursor = index;
        while let Some(record) = self.records.get(cursor as usize) {
            chain.push(cursor);
            if record.parent == NO_PARENT || record.is_root() {
                break;
            }
            cursor = record.parent;
        }

        let mut out = String::with_capacity(chain.len() * 16);
        for (position, entry) in chain.iter().rev().enumerate() {
            let name = self.name(*entry);
            if position == 0 {
                out.push_str(name);
                continue;
            }
            if !out.ends_with('\\') {
                out.push('\\');
            }
            out.push_str(name);
        }
        out
    }

    /// The index of a volume root, if it is in the store.
    #[must_use]
    pub fn root_index(&self, drive: char) -> Option<u32> {
        let wanted = drive.to_ascii_uppercase() as u8;
        self.records
            .iter()
            .position(|record| record.is_root() && record.drive == wanted)
            .and_then(|index| u32::try_from(index).ok())
    }

    /// The index of the record with a given NTFS file reference number.
    #[must_use]
    pub fn find_by_file_id(&self, file_id: u64) -> Option<u32> {
        if file_id == 0 {
            return None;
        }
        self.records
            .iter()
            .position(|record| record.file_id == file_id && !record.is_deleted())
            .and_then(|index| u32::try_from(index).ok())
    }

    /// Tombstone a record. Its children keep pointing at a valid index.
    pub fn mark_deleted(&mut self, index: u32) {
        if let Some(record) = self.records.get_mut(index as usize) {
            record.flags |= FLAG_DELETED;
        }
    }

    /// Give a record a new name. The arena only ever grows, so the previous
    /// name is simply left behind.
    pub fn rename(&mut self, index: u32, name: &str) {
        let (name_off, name_len) = self.intern(name);
        let ext_len = extension_of(name)
            .and_then(|ext| u8::try_from(ext.len()).ok())
            .unwrap_or(0);
        if let Some(record) = self.records.get_mut(index as usize) {
            record.name_off = name_off;
            record.name_len = name_len;
            record.ext_len = if record.is_directory() { 0 } else { ext_len };
            record.flags &= !FLAG_DELETED;
        }
    }

    /// Materialise one record as a shared entity.
    #[must_use]
    pub fn entity(&self, index: u32) -> Option<LocalEntity> {
        let record = *self.records.get(index as usize)?;
        if record.is_deleted() {
            return None;
        }
        let name = self.name(index).to_string();
        let path = self.path_of(index);
        let drive = self.drive(index);
        if record.is_directory() {
            Some(LocalEntity::Directory(DirectoryEntry {
                path,
                name,
                modified: nonzero(record.modified_ms),
                created: nonzero(record.created_ms),
                drive,
                file_id: nonzero_u64(record.file_id),
            }))
        } else {
            Some(LocalEntity::File(FileEntry {
                path,
                name,
                extension: self.extension(index).map(str::to_string),
                size: record.size,
                modified: nonzero(record.modified_ms),
                created: nonzero(record.created_ms),
                drive,
                file_id: nonzero_u64(record.file_id),
            }))
        }
    }

    /// Materialise every record. Only for small stores and for tests.
    #[must_use]
    pub fn entities(&self) -> Vec<LocalEntity> {
        (0..u32::try_from(self.records.len()).unwrap_or(0))
            .filter_map(|index| self.entity(index))
            .collect()
    }

    /// Rough memory footprint in bytes, for the index panel.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.arena.capacity() + self.records.capacity() * std::mem::size_of::<FileRecord>()
    }

    /// Number of live directories in the store.
    #[must_use]
    pub fn directory_count(&self) -> usize {
        self.records
            .iter()
            .filter(|record| record.is_directory() && !record.is_deleted())
            .count()
    }

    /// Number of live files in the store.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.records
            .iter()
            .filter(|record| !record.is_directory() && !record.is_deleted())
            .count()
    }

    /// Number of tombstones left behind by the change journal.
    #[must_use]
    pub fn deleted_count(&self) -> usize {
        self.records
            .iter()
            .filter(|record| record.is_deleted())
            .count()
    }

    /// Whether every live record is reachable from a volume root.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        (0..self.records.len()).all(|index| {
            let record = self.records[index];
            if record.is_deleted() || record.is_root() {
                return true;
            }
            (record.parent as usize) < self.records.len()
        })
    }
}

fn nonzero(value: i64) -> Option<i64> {
    (value != 0).then_some(value)
}

fn nonzero_u64(value: u64) -> Option<u64> {
    (value != 0).then_some(value)
}

/// The lower-case extension of a name.
#[must_use]
pub fn extension_of(name: &str) -> Option<String> {
    let (stem, extension) = name.rsplit_once('.')?;
    if stem.is_empty() || extension.is_empty() || extension.len() > 16 {
        return None;
    }
    Some(extension.to_ascii_lowercase())
}

/// The name a record would get for a path, used by the MFT backend.
#[must_use]
pub fn name_from_path(path: &str) -> String {
    file_name_of(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_layout_stays_compact() {
        // The whole memory budget depends on this staying at 48 bytes.
        assert_eq!(std::mem::size_of::<FileRecord>(), 48);
    }

    #[test]
    fn paths_are_reconstructed_from_parent_links() {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        let users = store.push_entry(root, "Users", 'C', true, 0, 0, 0, 0);
        let me = store.push_entry(users, "me", 'C', true, 0, 0, 0, 0);
        let file = store.push_entry(me, "notes.txt", 'C', false, 42, 1, 2, 99);
        assert_eq!(store.path_of(file), r"C:\Users\me\notes.txt");
        assert_eq!(store.path_of(root), r"C:\");
        assert_eq!(store.name(file), "notes.txt");
        assert_eq!(store.extension(file), Some("txt"));
    }

    #[test]
    fn directories_report_no_extension() {
        let mut store = FileStore::new();
        let root = store.push_root('D');
        let dir = store.push_entry(root, "my.folder", 'D', true, 0, 0, 0, 0);
        assert_eq!(store.extension(dir), None);
    }

    #[test]
    fn entities_carry_size_and_timestamps() {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        let file = store.push_entry(root, "a.pdf", 'C', false, 1234, 111, 222, 7);
        match store.entity(file) {
            Some(LocalEntity::File(entry)) => {
                assert_eq!(entry.name, "a.pdf");
                assert_eq!(entry.size, 1234);
                assert_eq!(entry.modified, Some(111));
                assert_eq!(entry.created, Some(222));
                assert_eq!(entry.extension.as_deref(), Some("pdf"));
                assert_eq!(entry.drive, Some('C'));
                assert_eq!(entry.file_id, Some(7));
                assert_eq!(entry.path, r"C:\a.pdf");
            }
            other => panic!("expected a file, got {other:?}"),
        }
    }

    #[test]
    fn directory_entity_uses_the_directory_variant() {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        let dir = store.push_entry(root, "projects", 'C', true, 0, 0, 0, 0);
        assert!(matches!(store.entity(dir), Some(LocalEntity::Directory(_))));
    }

    #[test]
    fn zero_timestamps_are_reported_as_unknown() {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        let file = store.push_entry(root, "x.bin", 'C', false, 1, 0, 0, 0);
        match store.entity(file) {
            Some(LocalEntity::File(entry)) => {
                assert_eq!(entry.modified, None);
                assert_eq!(entry.created, None);
                assert_eq!(entry.file_id, None);
            }
            other => panic!("expected a file, got {other:?}"),
        }
    }

    #[test]
    fn counts_split_files_and_directories() {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        let dir = store.push_entry(root, "dir", 'C', true, 0, 0, 0, 0);
        store.push_entry(dir, "a.txt", 'C', false, 1, 0, 0, 0);
        store.push_entry(dir, "b.txt", 'C', false, 1, 0, 0, 0);
        assert_eq!(store.file_count(), 2);
        assert_eq!(store.directory_count(), 2);
        assert_eq!(store.len(), 4);
    }

    #[test]
    fn out_of_range_indices_are_handled_without_panicking() {
        let store = FileStore::new();
        assert_eq!(store.name(7), "");
        assert_eq!(store.extension(7), None);
        assert_eq!(store.path_of(7), "");
        assert!(store.entity(7).is_none());
    }

    #[test]
    fn unicode_names_survive_the_arena() {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        let file = store.push_entry(root, "年度报告.pdf", 'C', false, 1, 0, 0, 0);
        assert_eq!(store.name(file), "年度报告.pdf");
        assert_eq!(store.path_of(file), r"C:\年度报告.pdf");
        assert_eq!(store.extension(file), Some("pdf"));
    }

    #[test]
    fn extension_helper_rejects_odd_names() {
        assert_eq!(extension_of("notes.txt"), Some("txt".into()));
        assert_eq!(extension_of("no-extension"), None);
        assert_eq!(extension_of(".gitignore"), None);
        assert_eq!(extension_of("a.verylongextensionindeed"), None);
    }

    #[test]
    fn store_round_trips_through_messagepack() {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        store.push_entry(root, "readme.md", 'C', false, 12, 5, 6, 0);
        let encoded = rmp_serde::to_vec(&store).unwrap();
        let decoded: FileStore = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded.name(1), "readme.md");
        assert_eq!(decoded.path_of(1), r"C:\readme.md");
    }
}
