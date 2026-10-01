//! The NTFS MFT + USN change journal backend.
//!
//! ```text
//!   \\.\C:  ---FSCTL_ENUM_USN_DATA---->  MFT records  --->  initial index
//!   \\.\C:  ---FSCTL_READ_USN_JOURNAL-->  journal      --->  incremental update
//! ```
//!
//! # What this gives you
//!
//! Enumeration reads the master file table through `FSCTL_ENUM_USN_DATA`
//! instead of walking directories, which is dramatically faster on a large
//! volume: names, parent references, attributes and timestamps for millions of
//! entries arrive in one sequential pass. After the initial build only the USN
//! change journal is read, so a warm start costs one journal read rather than
//! a rescan.
//!
//! # What it costs
//!
//! * **Administrator rights.** Opening `\\.\C:` for `FILE_READ_DATA` requires
//!   elevation. Without it the caller falls back to the directory scan
//!   backend. That is not a limitation of this implementation; it is the
//!   Windows security model.
//! * **No file sizes.** `USN_RECORD_V2` does not carry a size, and reading one
//!   needs a per-file `FSCTL_GET_NTFS_FILE_RECORD` plus NTFS attribute parsing.
//!   Entries from this backend therefore report a size of zero and `size:`
//!   filters do not match them. The directory scan backend is the default for
//!   exactly this reason.
//! * **NTFS only.** ReFS and FAT volumes are rejected and scanned instead.

use std::collections::HashMap;

use search_core::{clock, LceError, PermissionError, PlatformError};
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::journal::{JournalRegistry, RebuildReason, SyncDecision, VolumeSyncStatus};
use crate::mutation::{ApplyReport, IndexMutation};
use crate::scan::{IndexConfig, IndexReport};
use crate::store::FileStore;
use crate::volume::{VolumeId, VolumeSpec};

/// `FSCTL_ENUM_USN_DATA`.
pub const FSCTL_ENUM_USN_DATA: u32 = 0x0009_00B3;
/// `FSCTL_READ_USN_JOURNAL`.
pub const FSCTL_READ_USN_JOURNAL: u32 = 0x0009_00BB;
/// `FSCTL_QUERY_USN_JOURNAL`.
pub const FSCTL_QUERY_USN_JOURNAL: u32 = 0x0009_00F4;

const ERROR_HANDLE_EOF: u32 = 38;

/// Bytes of leading cursor before the first `USN_RECORD_V2`.
pub const USN_RECORD_OFFSET: usize = 8;
/// Size of the fixed `USN_RECORD_V2` header, before the file name.
pub const USN_RECORD_HEADER_LEN: usize = 60;

/// `MFT_ENUM_DATA_V0`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MftEnumDataV0 {
    /// Resume point; zero starts at the beginning of the MFT.
    pub start_file_reference_number: u64,
    /// Lowest USN to include.
    pub low_usn: i64,
    /// Highest USN to include; `i64::MAX` means "everything".
    pub high_usn: i64,
}

/// `READ_USN_JOURNAL_DATA_V0`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ReadUsnJournalDataV0 {
    /// Where to start reading.
    pub start_usn: i64,
    /// Reason bitmask; `u32::MAX` requests every record.
    pub reason_mask: u32,
    /// Whether to only return records on close.
    pub return_only_on_close: u32,
    /// Timeout in 100 ns units.
    pub timeout: u64,
    /// Bytes to wait for before returning.
    pub bytes_to_wait_for: u64,
    /// Journal identifier from [`QueryUsnJournalData`].
    pub usn_journal_id: u64,
}

impl Default for ReadUsnJournalDataV0 {
    fn default() -> Self {
        Self {
            start_usn: 0,
            reason_mask: u32::MAX,
            return_only_on_close: 0,
            timeout: 0,
            bytes_to_wait_for: 0,
            usn_journal_id: 0,
        }
    }
}

/// `USN_JOURNAL_DATA_V0`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryUsnJournalData {
    /// Journal identifier; changes if the journal is recreated.
    pub usn_journal_id: u64,
    /// Oldest readable USN.
    pub first_usn: i64,
    /// Next USN to be written.
    pub next_usn: i64,
    /// Lowest valid USN.
    pub lowest_valid_usn: i64,
    /// Maximum USN.
    pub max_usn: i64,
    /// Maximum journal size.
    pub maximum_size: u64,
    /// Allocation delta.
    pub allocation_delta: u64,
}

/// One parsed `USN_RECORD_V2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsnRecord {
    /// File reference number (MFT record plus sequence).
    pub file_reference_number: u64,
    /// Parent directory's file reference number.
    pub parent_file_reference_number: u64,
    /// Update sequence number.
    pub usn: i64,
    /// Timestamp as Unix milliseconds.
    pub timestamp_ms: i64,
    /// `USN_REASON_*` bit flags.
    pub reason: u32,
    /// Win32 file attributes.
    pub file_attributes: u32,
    /// File name.
    pub name: String,
}

/// `USN_REASON_DATA_OVERWRITE`.
pub const USN_REASON_DATA_OVERWRITE: u32 = 0x0000_0001;
/// `USN_REASON_DATA_EXTEND`.
pub const USN_REASON_DATA_EXTEND: u32 = 0x0000_0002;
/// `USN_REASON_DATA_TRUNCATION`.
pub const USN_REASON_DATA_TRUNCATION: u32 = 0x0000_0004;
/// Any of the data-change reason bits, which mean "contents differ now".
pub const USN_REASON_DATA_ANY: u32 =
    USN_REASON_DATA_OVERWRITE | USN_REASON_DATA_EXTEND | USN_REASON_DATA_TRUNCATION;
/// `USN_REASON_FILE_CREATE`.
pub const USN_REASON_FILE_CREATE: u32 = 0x0000_0100;
/// `USN_REASON_FILE_DELETE`.
pub const USN_REASON_FILE_DELETE: u32 = 0x0000_0200;
/// `USN_REASON_RENAME_OLD_NAME`.
pub const USN_REASON_RENAME_OLD_NAME: u32 = 0x0000_1000;
/// `USN_REASON_RENAME_NEW_NAME`.
pub const USN_REASON_RENAME_NEW_NAME: u32 = 0x0000_2000;
/// `FILE_ATTRIBUTE_DIRECTORY`.
pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;

impl UsnRecord {
    /// Whether this record describes a directory.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.file_attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }

    /// Whether the entry was created.
    #[must_use]
    pub const fn is_create(&self) -> bool {
        self.reason & USN_REASON_FILE_CREATE != 0
    }

    /// Whether the entry was deleted.
    #[must_use]
    pub const fn is_delete(&self) -> bool {
        self.reason & USN_REASON_FILE_DELETE != 0
    }

    /// Whether the entry was renamed (the record carries the new name).
    #[must_use]
    pub const fn is_rename(&self) -> bool {
        self.reason & USN_REASON_RENAME_NEW_NAME != 0
    }
}

/// Parse a device IO buffer that starts with an 8 byte cursor followed by
/// `USN_RECORD_V2` entries.
///
/// Malformed or truncated records stop the scan rather than producing garbage:
/// the caller simply rebuilds from the last known good USN.
#[must_use]
pub fn parse_usn_records(buffer: &[u8]) -> Vec<UsnRecord> {
    let mut records = Vec::new();
    let mut offset = USN_RECORD_OFFSET;
    while offset + USN_RECORD_HEADER_LEN <= buffer.len() {
        let Some((length, record)) = parse_record(&buffer[offset..]) else {
            break;
        };
        if length < USN_RECORD_HEADER_LEN as u32 {
            break;
        }
        records.push(record);
        offset = match offset.checked_add(length as usize) {
            Some(next) if next <= buffer.len() => next,
            _ => break,
        };
    }
    records
}

fn parse_record(bytes: &[u8]) -> Option<(u32, UsnRecord)> {
    if bytes.len() < USN_RECORD_HEADER_LEN {
        return None;
    }
    let record_length = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
    let file_reference_number = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let parent_file_reference_number = u64::from_le_bytes(bytes[16..24].try_into().ok()?);
    let usn = i64::from_le_bytes(bytes[24..32].try_into().ok()?);
    let timestamp = i64::from_le_bytes(bytes[32..40].try_into().ok()?);
    let reason = u32::from_le_bytes(bytes[40..44].try_into().ok()?);
    let file_attributes = u32::from_le_bytes(bytes[52..56].try_into().ok()?);
    let name_length = u16::from_le_bytes(bytes[56..58].try_into().ok()?);
    let name_offset = u16::from_le_bytes(bytes[58..60].try_into().ok()?);

    let start = usize::from(name_offset);
    let end = start.checked_add(usize::from(name_length))?;
    if end > bytes.len() || name_length % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes[start..end]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();

    Some((
        record_length,
        UsnRecord {
            file_reference_number,
            parent_file_reference_number,
            usn,
            timestamp_ms: filetime_to_unix_ms(timestamp),
            reason,
            file_attributes,
            name: String::from_utf16_lossy(&units),
        },
    ))
}

/// Convert a `FILETIME` tick count to Unix milliseconds.
#[must_use]
pub const fn filetime_to_unix_ms(ticks: i64) -> i64 {
    const EPOCH_DELTA_TICKS: i64 = 116_444_736_000_000_000;
    if ticks == 0 {
        return 0;
    }
    (ticks - EPOCH_DELTA_TICKS) / 10_000
}
/// The result of an MFT build.
#[derive(Debug)]
pub struct MftOutcome {
    /// The built index.
    pub store: FileStore,
    /// What the build did.
    pub report: IndexReport,
    /// Per-volume journal cursors, ready to persist.
    pub journals: JournalRegistry,
}

/// How a journal read should behave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalReadOptions {
    /// Whether to block until the journal has something new.
    ///
    /// This is what makes a worker event-driven rather than a poll loop: the
    /// kernel holds the call open until `bytes_to_wait_for` bytes have been
    /// written or `timeout_ms` has elapsed.
    pub wait: bool,
    /// Milliseconds to wait when `wait` is set.
    pub timeout_ms: u64,
    /// Bytes that must accumulate before the call returns when `wait` is set.
    pub bytes_to_wait_for: u64,
    /// Safety valve: stop after this many journal reads for one volume.
    pub max_reads: usize,
}

impl Default for JournalReadOptions {
    fn default() -> Self {
        Self {
            wait: false,
            timeout_ms: 0,
            bytes_to_wait_for: 0,
            max_reads: 64,
        }
    }
}

impl JournalReadOptions {
    /// Options for a blocking tail with a one second wake-up.
    #[must_use]
    pub const fn tailing() -> Self {
        Self {
            wait: true,
            timeout_ms: 1_000,
            bytes_to_wait_for: 1,
            max_reads: 64,
        }
    }
}

/// Everything one volume's journal read produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeBatch {
    /// Stable volume identity.
    pub volume_id: VolumeId,
    /// Label for logs and the status panel.
    pub label: String,
    /// Drive letter the volume is currently mounted on.
    pub drive: char,
    /// Sync state after the read.
    pub status: VolumeSyncStatus,
    /// Mutations to apply, in journal order.
    pub mutations: Vec<IndexMutation>,
    /// Cursor to store once the mutations are applied.
    pub next_usn: i64,
    /// Journal records examined.
    pub examined: usize,
    /// Set when the volume cannot be caught up incrementally.
    pub rebuild: Option<RebuildReason>,
}

impl VolumeBatch {
    /// Whether there is anything to do.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.rebuild.is_none() && self.mutations.is_empty()
    }
}

/// The result of reading every volume's journal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JournalBatch {
    /// One entry per NTFS volume that was read.
    pub volumes: Vec<VolumeBatch>,
    /// Problems that did not stop the read.
    pub warnings: Vec<String>,
}

impl JournalBatch {
    /// Total mutations across every volume.
    #[must_use]
    pub fn mutation_count(&self) -> usize {
        self.volumes.iter().map(|batch| batch.mutations.len()).sum()
    }

    /// Volumes that need a full rebuild.
    #[must_use]
    pub fn rebuilds(&self) -> Vec<&VolumeBatch> {
        self.volumes
            .iter()
            .filter(|batch| batch.rebuild.is_some())
            .collect()
    }

    /// Whether every volume is up to date.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.volumes.iter().all(VolumeBatch::is_noop)
    }
}

/// What an incremental update did.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReport {
    /// Journal records examined across every volume.
    pub examined: usize,
    /// Mutations applied to the store.
    pub applied: ApplyReport,
    /// Per-volume outcome, so one bad disk cannot hide behind a total.
    pub volumes: Vec<VolumeUpdateReport>,
    /// Whether any volume needs a full rebuild.
    pub requires_rebuild: bool,
}

/// One volume's contribution to an update.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeUpdateReport {
    /// Label of the volume.
    pub volume: String,
    /// Sync state after the update.
    pub status: VolumeSyncStatus,
    /// Journal records examined.
    pub examined: usize,
    /// What was applied.
    pub applied: ApplyReport,
    /// Why a rebuild is needed, if one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebuild: Option<RebuildReason>,
}

/// Whether the MFT backend can be used on this machine at all.
#[must_use]
pub fn is_available(volumes: &[VolumeSpec]) -> bool {
    volumes
        .iter()
        .filter(|volume| volume.is_ntfs)
        .any(|volume| open_volume(volume.drive, GENERIC_READ).is_some())
}

/// Build an index from the MFT of every NTFS volume in `config`.
///
/// Returns [`LceError::Permission`] when no volume handle can be opened; the
/// caller is expected to fall back to the scan backend.
pub fn build(config: &IndexConfig) -> Result<MftOutcome, LceError> {
    let started = std::time::Instant::now();
    let mut store = FileStore::new();
    let mut journals = JournalRegistry::new();
    let mut warnings = Vec::new();
    let mut indexed_volumes = Vec::new();
    let mut truncated = false;

    for volume in config.volumes.iter().filter(|volume| volume.is_ntfs) {
        match build_volume(volume, config, &mut store, &mut journals) {
            Ok(true) => indexed_volumes.push(volume.label()),
            Ok(false) => {
                indexed_volumes.push(volume.label());
                truncated = true;
            }
            Err(error) => {
                warnings.push(format!("{}: {}", volume.label(), error.hint()));
                if indexed_volumes.is_empty() {
                    return Err(error);
                }
            }
        }
    }

    if indexed_volumes.is_empty() {
        return Err(LceError::Platform(PlatformError::Unsupported {
            feature: "NTFS master file table enumeration".into(),
        }));
    }

    Ok(MftOutcome {
        report: IndexReport {
            backend: "mft-usn".into(),
            volumes: indexed_volumes,
            entries: store.len(),
            directories: store.directory_count(),
            truncated,
            elapsed_ms: elapsed_ms(started),
            warnings,
        },
        store,
        journals,
    })
}

fn build_volume(
    volume: &VolumeSpec,
    config: &IndexConfig,
    store: &mut FileStore,
    journals: &mut JournalRegistry,
) -> Result<bool, LceError> {
    let handle = open_volume(volume.drive, GENERIC_READ).ok_or_else(|| {
        LceError::Permission(PermissionError::VolumeAccessDenied {
            volume: volume.label(),
        })
    })?;

    let result = enumerate_volume(handle, volume, config, store, journals);
    // SAFETY: `handle` came from CreateFileW and is not used again.
    unsafe { CloseHandle(handle) };
    result
}

fn enumerate_volume(
    handle: HANDLE,
    volume: &VolumeSpec,
    config: &IndexConfig,
    store: &mut FileStore,
    journals: &mut JournalRegistry,
) -> Result<bool, LceError> {
    let mut entries: HashMap<u64, RawEntry> = HashMap::new();
    let mut start = MftEnumDataV0 {
        start_file_reference_number: 0,
        low_usn: 0,
        high_usn: i64::MAX,
    };
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut truncated = false;

    loop {
        let mut returned = 0u32;
        // SAFETY: the input and output buffers are valid for the lengths passed.
        let ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_ENUM_USN_DATA,
                std::ptr::addr_of_mut!(start).cast(),
                u32::try_from(std::mem::size_of::<MftEnumDataV0>()).unwrap_or(24),
                buffer.as_mut_ptr().cast(),
                u32::try_from(buffer.len()).unwrap_or(u32::MAX),
                &mut returned,
                std::ptr::null_mut(),
            )
        };

        if ok == 0 {
            let code = last_error();
            if code == ERROR_HANDLE_EOF {
                break;
            }
            return Err(LceError::Platform(PlatformError::WindowsApi {
                call: "DeviceIoControl(FSCTL_ENUM_USN_DATA)".into(),
                code: i64::from(code),
            }));
        }

        let returned = returned as usize;
        if returned <= USN_RECORD_OFFSET {
            break;
        }

        for record in parse_usn_records(&buffer[..returned]) {
            if entries.len() >= config.max_entries {
                truncated = true;
                break;
            }
            if record.name.is_empty() || record.name == "." {
                continue;
            }
            entries.insert(
                record.file_reference_number,
                RawEntry {
                    parent: record.parent_file_reference_number,
                    name: record.name,
                    file_attributes: record.file_attributes,
                    timestamp_ms: record.timestamp_ms,
                },
            );
        }
        if truncated {
            break;
        }

        let next = u64::from_le_bytes(
            buffer[..USN_RECORD_OFFSET]
                .try_into()
                .unwrap_or([0u8; USN_RECORD_OFFSET]),
        );
        if next == start.start_file_reference_number {
            break;
        }
        start.start_file_reference_number = next;
    }

    materialise(volume.drive, &entries, store);

    // Register the volume and adopt its journal. The cursor comes from a query
    // made *after* the enumeration so that anything created while the MFT was
    // being walked is replayed by the next incremental read rather than being
    // silently missed; replaying a create for a record that is already indexed
    // is a cheap no-op.
    journals.register(&volume.identity);
    match query_journal(handle) {
        Ok(journal) => {
            journals.observe(&volume.identity.id, &journal);
            let entries = store_entries_for_drive(store, volume.drive);
            journals.advance(&volume.identity.id, journal.next_usn, entries);
        }
        Err(error) => {
            tracing::warn!(
                volume = volume.label(),
                code = error.code(),
                "the change journal is unavailable; this volume needs a manual rebuild"
            );
            journals.mark_status(
                &volume.identity.id,
                VolumeSyncStatus::Stale,
                Some("the change journal could not be queried".into()),
            );
        }
    }

    Ok(!truncated)
}

/// Live records attributed to one drive letter.
fn store_entries_for_drive(store: &FileStore, drive: char) -> usize {
    let wanted = drive.to_ascii_uppercase() as u8;
    store
        .records()
        .iter()
        .filter(|record| !record.is_deleted() && record.drive == wanted)
        .count()
}
/// Turn the flat FRN graph into a store with resolvable parent links.
fn materialise(drive: char, entries: &HashMap<u64, RawEntry>, store: &mut FileStore) {
    let mut index_of: HashMap<u64, u32> = HashMap::new();
    for frn in entries.keys() {
        let _ = ensure_index(*frn, drive, entries, &mut index_of, store);
    }
}

fn ensure_index(
    frn: u64,
    drive: char,
    entries: &HashMap<u64, RawEntry>,
    index_of: &mut HashMap<u64, u32>,
    store: &mut FileStore,
) -> Option<u32> {
    if let Some(index) = index_of.get(&frn) {
        return Some(*index);
    }

    // Walk up until we reach either an already-materialised entry or a root.
    let mut chain: Vec<u64> = Vec::new();
    let mut cursor = frn;
    let mut base: Option<u32> = None;
    loop {
        if let Some(index) = index_of.get(&cursor).copied() {
            base = Some(index);
            break;
        }
        let entry = entries.get(&cursor)?;
        chain.push(cursor);
        if entry.parent == cursor || entry.parent == 0 {
            break;
        }
        if chain.len() > 1_024 {
            // A cycle in the parent graph; give up on this branch rather than
            // looping forever.
            return None;
        }
        cursor = entry.parent;
    }

    for candidate in chain.iter().rev() {
        let entry = entries.get(candidate)?;
        let index = match base {
            Some(parent) => store.push_entry(
                parent,
                &entry.name,
                drive,
                entry.is_directory(),
                0,
                entry.timestamp_ms,
                0,
                *candidate,
            ),
            // Only the volume root reaches this branch.
            None => store.push_root(drive),
        };
        index_of.insert(*candidate, index);
        base = Some(index);
    }

    index_of.get(&frn).copied()
}

#[derive(Debug, Clone)]
struct RawEntry {
    parent: u64,
    name: String,
    file_attributes: u32,
    timestamp_ms: i64,
}

impl RawEntry {
    fn is_directory(&self) -> bool {
        self.file_attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }
}

/// Read the journal position of a volume, if it has one.
fn query_journal(handle: HANDLE) -> Result<QueryUsnJournalData, LceError> {
    let mut data = QueryUsnJournalData::default();
    let mut returned = 0u32;
    // SAFETY: the output buffer is valid for the length passed.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_QUERY_USN_JOURNAL,
            std::ptr::null_mut(),
            0,
            std::ptr::addr_of_mut!(data).cast(),
            u32::try_from(std::mem::size_of::<QueryUsnJournalData>()).unwrap_or(56),
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(LceError::Platform(PlatformError::WindowsApi {
            call: "DeviceIoControl(FSCTL_QUERY_USN_JOURNAL)".into(),
            code: i64::from(last_error()),
        }));
    }
    Ok(data)
}

/// Read every volume's change journal and turn what changed into mutations.
///
/// Disk I/O and parsing happen here, deliberately **outside** any lock the
/// caller holds. The caller applies the returned batch under a short write
/// lock, so a slow or wedged volume can never block a search.
#[must_use]
pub fn read_journal_batches(
    volumes: &[VolumeSpec],
    journals: &mut JournalRegistry,
    options: JournalReadOptions,
) -> JournalBatch {
    let mut batch = JournalBatch::default();

    for volume in volumes.iter().filter(|volume| volume.is_ntfs) {
        journals.register(&volume.identity);
        let cursor = journals
            .get(&volume.identity.id)
            .map_or(0, |state| state.cursor_usn);

        let Some(handle) = open_volume(volume.drive, GENERIC_READ | GENERIC_WRITE) else {
            journals.mark_permission_denied(
                &volume.identity.id,
                "the volume handle could not be opened",
            );
            batch.warnings.push(format!(
                "{}: the change journal needs administrator rights",
                volume.label()
            ));
            batch.volumes.push(VolumeBatch {
                volume_id: volume.identity.id.clone(),
                label: volume.label(),
                drive: volume.drive,
                status: VolumeSyncStatus::PermissionDenied,
                mutations: Vec::new(),
                next_usn: cursor,
                examined: 0,
                rebuild: None,
            });
            continue;
        };

        let outcome = read_volume_journal(handle, volume, journals, options);
        // SAFETY: `handle` came from CreateFileW and is not used afterwards.
        unsafe { CloseHandle(handle) };

        match outcome {
            Ok(volume_batch) => batch.volumes.push(volume_batch),
            Err(error) => {
                tracing::warn!(
                    volume = volume.label(),
                    code = error.code(),
                    %error,
                    "change journal read failed"
                );
                journals.mark_status(
                    &volume.identity.id,
                    VolumeSyncStatus::Stale,
                    Some(error.hint().to_string()),
                );
                batch
                    .warnings
                    .push(format!("{}: {}", volume.label(), error.hint()));
                batch.volumes.push(VolumeBatch {
                    volume_id: volume.identity.id.clone(),
                    label: volume.label(),
                    drive: volume.drive,
                    status: VolumeSyncStatus::Stale,
                    mutations: Vec::new(),
                    next_usn: cursor,
                    examined: 0,
                    rebuild: Some(RebuildReason::ReadFailed),
                });
            }
        }
    }

    batch
}

fn read_volume_journal(
    handle: HANDLE,
    volume: &VolumeSpec,
    journals: &mut JournalRegistry,
    options: JournalReadOptions,
) -> Result<VolumeBatch, LceError> {
    let journal = query_journal(handle)?;
    let decision = journals.observe(&volume.identity.id, &journal);

    let mut batch = VolumeBatch {
        volume_id: volume.identity.id.clone(),
        label: volume.label(),
        drive: volume.drive,
        status: VolumeSyncStatus::CatchingUp,
        mutations: Vec::new(),
        next_usn: journal.next_usn,
        examined: 0,
        rebuild: None,
    };

    let start_usn = match decision {
        SyncDecision::UpToDate => {
            batch.status = VolumeSyncStatus::Healthy;
            if !options.wait {
                // Nothing to do and nobody asked to block: the common
                // "already caught up" case must cost exactly one ioctl.
                return Ok(batch);
            }
            journal.next_usn
        }
        SyncDecision::Rebuild { reason } => {
            batch.status = VolumeSyncStatus::Stale;
            batch.rebuild = Some(reason);
            return Ok(batch);
        }
        SyncDecision::Incremental { from_usn, .. } => from_usn,
    };

    let mut request = ReadUsnJournalDataV0 {
        start_usn,
        usn_journal_id: journal.usn_journal_id,
        timeout: if options.wait {
            options.timeout_ms.saturating_mul(10_000)
        } else {
            0
        },
        bytes_to_wait_for: if options.wait {
            options.bytes_to_wait_for
        } else {
            0
        },
        ..ReadUsnJournalDataV0::default()
    };
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut next_usn = start_usn;
    let mut reads = 0usize;

    loop {
        reads += 1;
        let mut returned = 0u32;
        // SAFETY: the input and output buffers are valid for the lengths passed.
        let ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_READ_USN_JOURNAL,
                std::ptr::addr_of_mut!(request).cast(),
                u32::try_from(std::mem::size_of::<ReadUsnJournalDataV0>()).unwrap_or(40),
                buffer.as_mut_ptr().cast(),
                u32::try_from(buffer.len()).unwrap_or(u32::MAX),
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(LceError::Platform(PlatformError::WindowsApi {
                call: "DeviceIoControl(FSCTL_READ_USN_JOURNAL)".into(),
                code: i64::from(last_error()),
            }));
        }

        let returned = returned as usize;
        if returned <= USN_RECORD_OFFSET {
            break;
        }

        let records = parse_usn_records(&buffer[..returned]);
        batch.examined += records.len();
        for record in &records {
            if let Some(mutation) = mutation_for(record, volume.drive) {
                batch.mutations.push(mutation);
            }
        }

        let next = u64::from_le_bytes(
            buffer[..USN_RECORD_OFFSET]
                .try_into()
                .unwrap_or([0u8; USN_RECORD_OFFSET]),
        );
        if next as i64 <= next_usn {
            break;
        }
        next_usn = next as i64;
        request.start_usn = next_usn;
        if reads >= options.max_reads {
            break;
        }
    }

    batch.next_usn = next_usn;
    Ok(batch)
}

/// Turn one journal record into an index mutation.
///
/// Returns `None` for records this index does not act on. `USN_RECORD_V2`
/// carries no file size, so a data change becomes a timestamp-only metadata
/// mutation and the stored size is left alone.
#[must_use]
pub fn mutation_for(record: &UsnRecord, drive: char) -> Option<IndexMutation> {
    if record.is_delete() {
        return Some(IndexMutation::Delete {
            file_id: record.file_reference_number,
        });
    }
    if record.is_create() {
        return Some(IndexMutation::Create {
            parent_file_id: record.parent_file_reference_number,
            file_id: record.file_reference_number,
            name: record.name.clone(),
            is_directory: record.is_directory(),
            drive,
            size: 0,
            modified_ms: record.timestamp_ms,
        });
    }
    if record.is_rename() {
        return Some(IndexMutation::Rename {
            file_id: record.file_reference_number,
            new_name: record.name.clone(),
        });
    }
    if record.reason & USN_REASON_DATA_ANY != 0 {
        return Some(IndexMutation::MetadataChanged {
            file_id: record.file_reference_number,
            size: None,
            modified_ms: record.timestamp_ms,
        });
    }
    None
}

/// Apply a journal batch to the store and advance the per-volume cursors.
///
/// The caller owns the write lock; this function performs no I/O of its own,
/// which is what keeps the lock hold time proportional to the batch rather than
/// to a disk round trip.
pub fn apply_journal_batch(
    store: &mut FileStore,
    journals: &mut JournalRegistry,
    batch: &JournalBatch,
) -> UpdateReport {
    let mut report = UpdateReport::default();

    for volume in &batch.volumes {
        let applied = store.apply_batch(&volume.mutations);
        report.examined += volume.examined;
        report.applied.merge(&applied);

        if let Some(reason) = volume.rebuild {
            report.requires_rebuild = true;
            journals.require_rebuild(&volume.volume_id, reason);
        } else {
            let entries = store_entries_for_drive(store, volume.drive);
            journals.advance(&volume.volume_id, volume.next_usn, entries);
        }

        report.volumes.push(VolumeUpdateReport {
            volume: volume.label.clone(),
            status: volume.status,
            examined: volume.examined,
            applied,
            rebuild: volume.rebuild,
        });
    }

    report
}
fn open_volume(drive: char, desired_access: u32) -> Option<HANDLE> {
    let path = format!(r"\\.\{drive}:");
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is NUL terminated; a failure returns INVALID_HANDLE_VALUE,
    // which is checked below.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            desired_access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    (handle != INVALID_HANDLE_VALUE && !handle.is_null()).then_some(handle)
}

fn last_error() -> u32 {
    // SAFETY: GetLastError is always safe to call.
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

fn elapsed_ms(started: std::time::Instant) -> f64 {
    let micros = started.elapsed().as_micros() as f64;
    (micros / 1_000.0).round() / 1_000.0
}

/// The USN reason bits this backend reacts to.
#[must_use]
pub const fn watched_reasons() -> u32 {
    USN_REASON_FILE_CREATE
        | USN_REASON_FILE_DELETE
        | USN_REASON_RENAME_OLD_NAME
        | USN_REASON_RENAME_NEW_NAME
}

/// The current wall-clock time in Unix milliseconds.
#[must_use]
pub fn now_ms() -> i64 {
    clock::now_ms()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::FileStore;

    /// Build a synthetic `USN_RECORD_V2` buffer with the leading cursor.
    fn synthetic_buffer(records: &[(u64, u64, &str, u32, u32)]) -> Vec<u8> {
        let mut buffer = vec![0u8; USN_RECORD_OFFSET];
        for (frn, parent, name, attributes, reason) in records {
            let name_units: Vec<u16> = name.encode_utf16().collect();
            let name_bytes = name_units.len() * 2;
            let record_length = USN_RECORD_HEADER_LEN + name_bytes;
            let mut record = vec![0u8; record_length];
            record[0..4].copy_from_slice(&(record_length as u32).to_le_bytes());
            record[4..6].copy_from_slice(&2u16.to_le_bytes());
            record[6..8].copy_from_slice(&0u16.to_le_bytes());
            record[8..16].copy_from_slice(&frn.to_le_bytes());
            record[16..24].copy_from_slice(&parent.to_le_bytes());
            record[24..32].copy_from_slice(&7i64.to_le_bytes());
            record[32..40].copy_from_slice(&116_444_736_000_000_000i64.to_le_bytes());
            record[40..44].copy_from_slice(&reason.to_le_bytes());
            record[44..48].copy_from_slice(&0u32.to_le_bytes());
            record[48..52].copy_from_slice(&0u32.to_le_bytes());
            record[52..56].copy_from_slice(&attributes.to_le_bytes());
            record[56..58].copy_from_slice(&(name_bytes as u16).to_le_bytes());
            record[58..60].copy_from_slice(&(USN_RECORD_HEADER_LEN as u16).to_le_bytes());
            for (index, unit) in name_units.iter().enumerate() {
                let start = USN_RECORD_HEADER_LEN + index * 2;
                record[start..start + 2].copy_from_slice(&unit.to_le_bytes());
            }
            buffer.extend_from_slice(&record);
        }
        buffer
    }

    fn raw(parent: u64, name: &str, directory: bool) -> RawEntry {
        RawEntry {
            parent,
            name: name.to_string(),
            file_attributes: if directory {
                FILE_ATTRIBUTE_DIRECTORY
            } else {
                0
            },
            timestamp_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn control_codes_match_the_documented_values() {
        assert_eq!(FSCTL_ENUM_USN_DATA, 0x0009_00B3);
        assert_eq!(FSCTL_READ_USN_JOURNAL, 0x0009_00BB);
        assert_eq!(FSCTL_QUERY_USN_JOURNAL, 0x0009_00F4);
    }

    #[test]
    fn struct_layouts_match_the_win32_headers() {
        assert_eq!(std::mem::size_of::<MftEnumDataV0>(), 24);
        // StartUsn(8) + ReasonMask(4) + ReturnOnlyOnClose(4) + Timeout(8)
        // + BytesToWaitFor(8) + UsnJournalID(8).
        assert_eq!(std::mem::size_of::<ReadUsnJournalDataV0>(), 40);
        assert_eq!(std::mem::size_of::<QueryUsnJournalData>(), 56);
    }

    #[test]
    fn a_single_record_round_trips() {
        let buffer = synthetic_buffer(&[(42, 5, "readme.md", 0, USN_REASON_FILE_CREATE)]);
        let records = parse_usn_records(&buffer);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].file_reference_number, 42);
        assert_eq!(records[0].parent_file_reference_number, 5);
        assert_eq!(records[0].name, "readme.md");
        assert_eq!(records[0].usn, 7);
        assert!(records[0].is_create());
        assert!(!records[0].is_directory());
    }

    #[test]
    fn multiple_records_are_walked_in_order() {
        let buffer = synthetic_buffer(&[
            (10, 5, "alpha.txt", 0, USN_REASON_FILE_CREATE),
            (
                11,
                5,
                "beta",
                FILE_ATTRIBUTE_DIRECTORY,
                USN_REASON_FILE_CREATE,
            ),
            (12, 11, "gamma.rs", 0, USN_REASON_RENAME_NEW_NAME),
        ]);
        let records = parse_usn_records(&buffer);
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].name, "alpha.txt");
        assert!(records[1].is_directory());
        assert!(records[2].is_rename());
    }

    #[test]
    fn unicode_names_survive_the_round_trip() {
        let buffer = synthetic_buffer(&[(7, 5, "年度报告.pdf", 0, USN_REASON_FILE_CREATE)]);
        let records = parse_usn_records(&buffer);
        assert_eq!(records[0].name, "年度报告.pdf");
    }

    #[test]
    fn a_truncated_record_stops_the_parse_without_panicking() {
        let mut buffer = synthetic_buffer(&[(1, 5, "ok.txt", 0, USN_REASON_FILE_CREATE)]);
        buffer.truncate(buffer.len() - 4);
        assert!(parse_usn_records(&buffer).is_empty());
    }

    #[test]
    fn an_empty_buffer_yields_no_records() {
        assert!(parse_usn_records(&[]).is_empty());
        assert!(parse_usn_records(&[0u8; USN_RECORD_OFFSET]).is_empty());
    }

    #[test]
    fn a_zero_length_record_is_rejected() {
        let mut buffer = vec![0u8; USN_RECORD_OFFSET + USN_RECORD_HEADER_LEN];
        buffer[USN_RECORD_OFFSET..USN_RECORD_OFFSET + 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(parse_usn_records(&buffer).is_empty());
    }

    #[test]
    fn the_parent_graph_is_materialised_into_paths() {
        let mut entries = HashMap::new();
        entries.insert(5u64, raw(5, "root", true));
        entries.insert(10, raw(5, "Users", true));
        entries.insert(11, raw(10, "notes.txt", false));

        let mut store = FileStore::new();
        materialise('C', &entries, &mut store);

        let paths: Vec<String> = (0..store.len() as u32).map(|i| store.path_of(i)).collect();
        assert!(paths.contains(&r"C:\".to_string()), "{paths:?}");
        assert!(paths.contains(&r"C:\Users".to_string()), "{paths:?}");
        assert!(
            paths.contains(&r"C:\Users\notes.txt".to_string()),
            "{paths:?}"
        );
        assert!(store.is_consistent());
    }

    #[test]
    fn a_child_listed_before_its_parent_is_still_materialised() {
        let mut entries = HashMap::new();
        entries.insert(5u64, raw(5, "root", true));
        entries.insert(99, raw(5, "late.txt", false));

        let mut store = FileStore::new();
        materialise('D', &entries, &mut store);
        let paths: Vec<String> = (0..store.len() as u32).map(|i| store.path_of(i)).collect();
        assert!(paths.contains(&r"D:\late.txt".to_string()), "{paths:?}");
    }

    #[test]
    fn a_missing_parent_is_skipped_rather_than_invented() {
        let mut entries = HashMap::new();
        entries.insert(77, raw(12_345, "orphan.txt", false));
        let mut store = FileStore::new();
        materialise('C', &entries, &mut store);
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn deep_chains_are_materialised_once() {
        let mut entries = HashMap::new();
        entries.insert(5u64, raw(5, "root", true));
        let mut parent = 5u64;
        for index in 0..40u64 {
            let frn = 100 + index;
            entries.insert(frn, raw(parent, &format!("level{index}"), true));
            parent = frn;
        }
        let mut store = FileStore::new();
        materialise('C', &entries, &mut store);
        assert_eq!(store.len(), 41);
        let deepest = store.path_of(store.len() as u32 - 1);
        assert!(deepest.contains("level39"), "{deepest}");
    }

    #[test]
    fn watch_reasons_cover_create_delete_and_rename() {
        let reasons = watched_reasons();
        assert_ne!(reasons & USN_REASON_FILE_CREATE, 0);
        assert_ne!(reasons & USN_REASON_FILE_DELETE, 0);
        assert_ne!(reasons & USN_REASON_RENAME_NEW_NAME, 0);
    }

    #[test]
    fn filetime_conversion_handles_edge_values() {
        assert_eq!(filetime_to_unix_ms(0), 0);
        assert_eq!(
            filetime_to_unix_ms(116_444_736_000_000_000 + 10_000_000),
            1_000
        );
    }

    #[test]
    fn tombstones_hide_records_without_breaking_child_indices() {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        let dir = store.push_entry(root, "dir", 'C', true, 0, 0, 0, 10);
        let child = store.push_entry(dir, "child.txt", 'C', false, 5, 0, 0, 11);
        store.mark_deleted(dir);

        assert!(store.entity(dir).is_none());
        assert_eq!(store.deleted_count(), 1);
        assert_eq!(store.directory_count(), 1);
        assert!(store.entity(child).is_some());
        assert_eq!(store.path_of(child), r"C:\dir\child.txt");
        assert!(store.is_consistent());
    }

    #[test]
    fn renaming_appends_a_new_name_without_moving_the_record() {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        let file = store.push_entry(root, "old.txt", 'C', false, 1, 0, 0, 0);
        store.rename(file, "new.pdf");
        assert_eq!(store.name(file), "new.pdf");
        assert_eq!(store.extension(file), Some("pdf"));
        assert_eq!(store.path_of(file), r"C:\new.pdf");
    }

    #[test]
    fn file_reference_numbers_are_findable() {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        let file = store.push_entry(root, "tracked.txt", 'C', false, 1, 0, 0, 424_242);
        assert_eq!(store.find_by_file_id(424_242), Some(file));
        assert_eq!(store.find_by_file_id(0), None);
        assert_eq!(store.find_by_file_id(999), None);
    }

    #[test]
    fn the_root_index_is_looked_up_by_drive() {
        let mut store = FileStore::new();
        let root = store.push_root('C');
        assert_eq!(store.root_index('C'), Some(root));
        assert_eq!(store.root_index('c'), Some(root));
        assert_eq!(store.root_index('Z'), None);
    }
}
