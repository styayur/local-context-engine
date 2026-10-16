//! Per-volume change-journal state.
//!
//! A single `Option<JournalState>` cannot describe a machine with more than one
//! NTFS volume: `C:` and `D:` have different journal identifiers, different USN
//! cursors, and can roll over independently. Losing that distinction is how you
//! end up serving stale results with a straight face.
//!
//! [`JournalRegistry`] is the fix. Every volume gets its own entry keyed by
//! [`VolumeId`] — never by drive letter — and every decision about
//! "can this volume catch up incrementally, or must it be rebuilt?" is made
//! per volume.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use search_core::clock;

use crate::mft::QueryUsnJournalData;
use crate::volume::{VolumeId, VolumeIdentity};

/// Bumped whenever the persisted shape of [`JournalRegistry`] changes.
///
/// A mismatch is not an error: the registry is discarded and every volume is
/// rebuilt. That is deliberately the *only* migration path, because guessing
/// at the meaning of an older cursor is how a stale index happens.
pub const JOURNAL_SCHEMA_VERSION: u32 = 2;

/// How trustworthy a volume's index currently is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VolumeSyncStatus {
    /// The cursor is current: mutations are being applied as they arrive.
    Healthy,
    /// The worker is applying a batch it already read.
    CatchingUp,
    /// The index is being rebuilt from the MFT. Results may be incomplete.
    #[default]
    Rebuilding,
    /// Incremental consistency could not be proven; a rebuild is required.
    Stale,
    /// The volume is not currently mounted.
    Offline,
    /// The volume exists but the journal cannot be opened without elevation.
    PermissionDenied,
}

impl VolumeSyncStatus {
    /// Lower-case wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            VolumeSyncStatus::Healthy => "healthy",
            VolumeSyncStatus::CatchingUp => "catching-up",
            VolumeSyncStatus::Rebuilding => "rebuilding",
            VolumeSyncStatus::Stale => "stale",
            VolumeSyncStatus::Offline => "offline",
            VolumeSyncStatus::PermissionDenied => "permission-denied",
        }
    }

    /// Whether results from this volume can be presented as current.
    #[must_use]
    pub const fn is_current(self) -> bool {
        matches!(
            self,
            VolumeSyncStatus::Healthy | VolumeSyncStatus::CatchingUp
        )
    }

    /// Whether the UI should warn about this volume.
    #[must_use]
    pub const fn is_degraded(self) -> bool {
        !self.is_current()
    }
}

impl std::fmt::Display for VolumeSyncStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One volume's journal position and health.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeJournalState {
    /// Stable identity of the volume this state describes.
    pub identity: VolumeIdentity,
    /// Journal identifier reported by `FSCTL_QUERY_USN_JOURNAL`.
    pub journal_id: u64,
    /// Oldest readable USN in the journal.
    pub first_usn: i64,
    /// Next USN the journal will write.
    pub next_usn: i64,
    /// How far this index has consumed the journal.
    pub cursor_usn: i64,
    /// Current health.
    pub status: VolumeSyncStatus,
    /// When the cursor last moved, in Unix milliseconds.
    pub last_sync_ms: i64,
    /// Why the volume is not [`VolumeSyncStatus::Healthy`], if it is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Indexed entries attributed to this volume, for the status panel.
    #[serde(default)]
    pub entries: usize,
    /// Whether a cursor has ever been established for this volume.
    ///
    /// This is an explicit flag rather than "cursor == 0", because USN 0 is a
    /// legal starting point and conflating the two is how a never-indexed
    /// volume gets mistaken for a caught-up one.
    #[serde(default)]
    pub has_cursor: bool,
    /// A rebuild the registry has already decided is necessary.
    ///
    /// Taken (and cleared) by the next [`JournalRegistry::observe`], so the
    /// reason survives until a worker acts on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_rebuild: Option<RebuildReason>,
}

impl VolumeJournalState {
    /// A brand new state that needs a full build.
    #[must_use]
    pub fn new(identity: VolumeIdentity, now_ms: i64) -> Self {
        Self {
            identity,
            journal_id: 0,
            first_usn: 0,
            next_usn: 0,
            cursor_usn: 0,
            status: VolumeSyncStatus::Rebuilding,
            last_sync_ms: now_ms,
            last_error: None,
            entries: 0,
            has_cursor: false,
            pending_rebuild: None,
        }
    }

    /// Mark that a rebuild is mandatory before this volume can be trusted.
    pub fn require_rebuild(&mut self, reason: RebuildReason) {
        self.pending_rebuild = Some(reason);
        self.status = VolumeSyncStatus::Stale;
        self.last_error = Some(reason.describe().to_string());
        self.has_cursor = false;
    }

    /// How far behind the journal this index is, in USN units.
    #[must_use]
    pub fn lag(&self) -> i64 {
        (self.next_usn - self.cursor_usn).max(0)
    }

    /// Whether the cursor is inside the journal's readable window.
    #[must_use]
    pub fn cursor_is_readable(&self) -> bool {
        self.cursor_usn >= self.first_usn && self.cursor_usn <= self.next_usn
    }
}

/// Why a volume cannot be caught up incrementally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RebuildReason {
    /// No state was stored for this volume.
    NewVolume,
    /// The journal was deleted, recreated or formatted.
    JournalRecreated,
    /// The saved cursor fell out of the journal's readable window.
    CursorBeforeFirstUsn,
    /// The journal is behind the saved cursor, which means the volume changed.
    CursorAheadOfJournal,
    /// The caller asked for a rebuild.
    Requested,
    /// An incremental read failed and could not be retried.
    ReadFailed,
}

impl RebuildReason {
    /// A short, user-facing explanation.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            RebuildReason::NewVolume => "the volume has not been indexed yet",
            RebuildReason::JournalRecreated => "the change journal was recreated",
            RebuildReason::CursorBeforeFirstUsn => "the change journal rolled over",
            RebuildReason::CursorAheadOfJournal => "the change journal moved backwards",
            RebuildReason::Requested => "a rebuild was requested",
            RebuildReason::ReadFailed => "the change journal could not be read",
        }
    }

    /// Whether the previous index content is still usable while rebuilding.
    #[must_use]
    pub const fn keeps_previous_entries(self) -> bool {
        matches!(
            self,
            RebuildReason::CursorBeforeFirstUsn | RebuildReason::JournalRecreated
        )
    }
}

/// What a worker should do for one volume, given what the journal currently
/// reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "kebab-case")]
pub enum SyncDecision {
    /// Nothing has changed since the cursor.
    UpToDate,
    /// Read the journal from `from_usn` and apply the mutations.
    Incremental {
        /// Exclusive USN to resume from.
        from_usn: i64,
        /// The journal will not report anything past this USN.
        up_to_usn: i64,
    },
    /// Incremental consistency cannot be proven; rebuild this volume.
    Rebuild {
        /// Why.
        reason: RebuildReason,
    },
}

impl SyncDecision {
    /// Whether this decision requires work.
    #[must_use]
    pub const fn is_noop(self) -> bool {
        matches!(self, SyncDecision::UpToDate)
    }
}

/// Every volume's journal state, keyed by stable volume identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRegistry {
    /// Persisted schema version.
    pub schema_version: u32,
    /// Per-volume state, keyed by the stored [`VolumeId`] string.
    ///
    /// The map is keyed by `String` rather than by `VolumeId` on purpose:
    /// MessagePack map keys have to be plain strings, and a newtype wrapper
    /// around `String` does not survive that round trip.
    pub volumes: BTreeMap<String, VolumeJournalState>,
}

impl Default for JournalRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl JournalRegistry {
    /// An empty, current-schema registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            volumes: BTreeMap::new(),
        }
    }

    /// Whether a persisted registry can be used as-is.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.schema_version == JOURNAL_SCHEMA_VERSION
    }

    /// How many volumes are tracked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.volumes.len()
    }

    /// Whether no volume is tracked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.volumes.is_empty()
    }

    /// Drop every volume and start over, keeping the schema version.
    pub fn clear(&mut self) {
        self.volumes.clear();
    }

    /// Register (or re-register) a volume, preserving any existing cursor.
    ///
    /// A drive-letter change keeps the cursor because the lookup is by
    /// [`VolumeId`]; only the mount points on the stored identity are updated.
    pub fn register(&mut self, identity: &VolumeIdentity) -> &mut VolumeJournalState {
        let now = clock::now_ms();
        let entry = self
            .volumes
            .entry(identity.id.as_str().to_string())
            .or_insert_with(|| VolumeJournalState::new(identity.clone(), now));
        // The identity is authoritative for mount points, which change.
        entry.identity.mount_points = identity.mount_points.clone();
        entry.identity.file_system = identity.file_system.clone();
        if identity.serial_number != 0 {
            entry.identity.serial_number = identity.serial_number;
        }
        entry
    }

    /// The state for a volume, if it is tracked.
    #[must_use]
    pub fn get(&self, volume_id: &VolumeId) -> Option<&VolumeJournalState> {
        self.volumes.get(volume_id.as_str())
    }

    /// A mutable view of a volume's state.
    pub fn get_mut(&mut self, volume_id: &VolumeId) -> Option<&mut VolumeJournalState> {
        self.volumes.get_mut(volume_id.as_str())
    }

    /// Forget a volume that no longer exists.
    pub fn remove(&mut self, volume_id: &VolumeId) -> Option<VolumeJournalState> {
        self.volumes.remove(volume_id.as_str())
    }

    /// Record that a volume is not mounted, without forgetting its cursor.
    pub fn mark_offline(&mut self, volume_id: &VolumeId) {
        if let Some(state) = self.volumes.get_mut(volume_id.as_str()) {
            state.status = VolumeSyncStatus::Offline;
        }
    }

    /// Record that the journal could not be opened.
    pub fn mark_permission_denied(&mut self, volume_id: &VolumeId, error: impl Into<String>) {
        if let Some(state) = self.volumes.get_mut(volume_id.as_str()) {
            state.status = VolumeSyncStatus::PermissionDenied;
            state.last_error = Some(error.into());
        }
    }

    /// Force a status and an optional explanation.
    pub fn mark_status(
        &mut self,
        volume_id: &VolumeId,
        status: VolumeSyncStatus,
        error: Option<String>,
    ) {
        if let Some(state) = self.volumes.get_mut(volume_id.as_str()) {
            state.status = status;
            state.last_error = error;
        }
    }

    /// Fold a fresh `FSCTL_QUERY_USN_JOURNAL` reading into the registry and
    /// decide what the worker should do next.
    ///
    /// This is the only place that is allowed to decide "incremental or
    /// rebuild", which is what keeps the rule identical for every volume.
    pub fn observe(&mut self, volume_id: &VolumeId, journal: &QueryUsnJournalData) -> SyncDecision {
        let now = clock::now_ms();
        let Some(state) = self.volumes.get_mut(volume_id.as_str()) else {
            return SyncDecision::Rebuild {
                reason: RebuildReason::NewVolume,
            };
        };

        state.first_usn = journal.first_usn;
        state.next_usn = journal.next_usn;
        state.last_sync_ms = now;

        // A decision the registry already made outranks anything the journal
        // says: honour it exactly once.
        if let Some(reason) = state.pending_rebuild.take() {
            return SyncDecision::Rebuild { reason };
        }

        // Never indexed, or a rebuild was forced and consumed earlier: the
        // cursor cannot be compared to anything.
        if !state.has_cursor {
            state.journal_id = journal.usn_journal_id;
            state.cursor_usn = journal.first_usn;
            state.status = VolumeSyncStatus::Rebuilding;
            return SyncDecision::Rebuild {
                reason: RebuildReason::NewVolume,
            };
        }

        // A recreated journal is a different journal, full stop.
        if state.journal_id != 0 && state.journal_id != journal.usn_journal_id {
            state.journal_id = journal.usn_journal_id;
            state.cursor_usn = journal.first_usn;
            state.status = VolumeSyncStatus::Stale;
            state.last_error = Some(RebuildReason::JournalRecreated.describe().to_string());
            return SyncDecision::Rebuild {
                reason: RebuildReason::JournalRecreated,
            };
        }

        if state.journal_id == 0 {
            state.journal_id = journal.usn_journal_id;
        }

        if state.cursor_usn < journal.first_usn {
            // Rollover: everything between the cursor and `first_usn` is gone,
            // so the index has a hole and must be rebuilt.
            state.cursor_usn = journal.first_usn;
            state.status = VolumeSyncStatus::Stale;
            state.last_error = Some(RebuildReason::CursorBeforeFirstUsn.describe().to_string());
            return SyncDecision::Rebuild {
                reason: RebuildReason::CursorBeforeFirstUsn,
            };
        }

        if state.cursor_usn > journal.next_usn {
            // The journal is behind the cursor: the volume was swapped, or the
            // journal was reset without changing its identifier.
            state.cursor_usn = journal.first_usn;
            state.status = VolumeSyncStatus::Stale;
            state.last_error = Some(RebuildReason::CursorAheadOfJournal.describe().to_string());
            return SyncDecision::Rebuild {
                reason: RebuildReason::CursorAheadOfJournal,
            };
        }

        if state.cursor_usn == journal.next_usn {
            state.status = VolumeSyncStatus::Healthy;
            state.last_error = None;
            return SyncDecision::UpToDate;
        }

        state.status = VolumeSyncStatus::CatchingUp;
        SyncDecision::Incremental {
            from_usn: state.cursor_usn,
            up_to_usn: journal.next_usn,
        }
    }

    /// Advance a volume's cursor after a batch was applied successfully.
    pub fn advance(&mut self, volume_id: &VolumeId, next_usn: i64, entries: usize) {
        if let Some(state) = self.volumes.get_mut(volume_id.as_str()) {
            state.cursor_usn = next_usn;
            state.last_sync_ms = clock::now_ms();
            state.entries = entries;
            state.has_cursor = true;
            state.pending_rebuild = None;
            if state.cursor_usn >= state.next_usn {
                state.status = VolumeSyncStatus::Healthy;
                state.last_error = None;
            } else {
                state.status = VolumeSyncStatus::CatchingUp;
            }
        }
    }

    /// Adopt the identity a volume reports now, returning whether the volume
    /// looked like a different disk.
    ///
    /// A drive-letter change must *not* look like a different disk: the caller
    /// passes the identity it computed from the mount point, and this compares
    /// it against what is stored under the same [`VolumeId`].
    pub fn reconcile_identity(&mut self, identity: &VolumeIdentity) -> IdentityReconciliation {
        match self.volumes.get_mut(identity.id.as_str()) {
            Some(state) => {
                let serial_changed = state.identity.serial_number != 0
                    && identity.serial_number != 0
                    && state.identity.serial_number != identity.serial_number;
                state.identity.mount_points = identity.mount_points.clone();
                state.identity.file_system = identity.file_system.clone();
                if serial_changed {
                    IdentityReconciliation::SerialChanged
                } else {
                    IdentityReconciliation::SameVolume
                }
            }
            None => IdentityReconciliation::Unknown,
        }
    }

    /// Every volume that is not currently healthy.
    #[must_use]
    pub fn degraded(&self) -> Vec<(VolumeId, VolumeSyncStatus)> {
        self.volumes
            .iter()
            .filter(|(_, state)| state.status.is_degraded())
            .map(|(id, state)| (VolumeId::new(id.clone()), state.status))
            .collect()
    }

    /// Whether every tracked volume is healthy.
    #[must_use]
    pub fn all_healthy(&self) -> bool {
        self.volumes.values().all(|state| state.status.is_current())
    }

    /// Total indexed entries across every volume.
    #[must_use]
    pub fn total_entries(&self) -> usize {
        self.volumes.values().map(|state| state.entries).sum()
    }

    /// Force one volume to rebuild on its next observation.
    pub fn require_rebuild(&mut self, volume_id: &VolumeId, reason: RebuildReason) {
        if let Some(state) = self.volumes.get_mut(volume_id.as_str()) {
            state.require_rebuild(reason);
        }
    }

    /// Discard the stored cursors so every volume rebuilds.
    pub fn invalidate_all(&mut self, reason: RebuildReason) {
        for state in self.volumes.values_mut() {
            state.cursor_usn = state.first_usn;
            state.require_rebuild(reason);
        }
    }
}

/// What happened when a stored identity met a freshly computed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityReconciliation {
    /// No stored state; the caller must build the index.
    Unknown,
    /// Same volume, possibly at a new mount point.
    SameVolume,
    /// The identifier matched but the serial number did not: treat with care.
    SerialChanged,
}

impl IdentityReconciliation {
    /// Whether the stored cursor can be reused.
    #[must_use]
    pub const fn cursor_is_reusable(self) -> bool {
        matches!(self, IdentityReconciliation::SameVolume)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(guid: &str, serial: u32, mount: &str) -> VolumeIdentity {
        let mut identity = VolumeIdentity::new(VolumeId::new(guid), serial, "NTFS");
        identity.add_mount_point(mount);
        identity
    }

    fn journal(id: u64, first: i64, next: i64) -> QueryUsnJournalData {
        QueryUsnJournalData {
            usn_journal_id: id,
            first_usn: first,
            next_usn: next,
            lowest_valid_usn: first,
            max_usn: next + 1_000,
            maximum_size: 32 * 1024 * 1024,
            allocation_delta: 4 * 1024 * 1024,
        }
    }

    #[test]
    fn an_unknown_volume_asks_for_a_rebuild() {
        let mut registry = JournalRegistry::new();
        let id = VolumeId::new(r"\\?\Volume{c}\");
        let decision = registry.observe(&id, &journal(1, 0, 100));
        assert_eq!(
            decision,
            SyncDecision::Rebuild {
                reason: RebuildReason::NewVolume
            }
        );
    }

    #[test]
    fn two_volumes_keep_independent_journal_ids_and_cursors() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0xC, r"C:\");
        let d = identity(r"\\?\Volume{d}\", 0xD, r"D:\");
        registry.register(&c);
        registry.register(&d);

        // Both start from scratch.
        assert!(matches!(
            registry.observe(&c.id, &journal(1000, 0, 500)),
            SyncDecision::Rebuild { .. }
        ));
        assert!(matches!(
            registry.observe(&d.id, &journal(2000, 0, 900)),
            SyncDecision::Rebuild { .. }
        ));
        registry.advance(&c.id, 500, 10);
        registry.advance(&d.id, 900, 20);

        assert_eq!(registry.get(&c.id).unwrap().journal_id, 1000);
        assert_eq!(registry.get(&d.id).unwrap().journal_id, 2000);
        assert_eq!(registry.get(&c.id).unwrap().cursor_usn, 500);
        assert_eq!(registry.get(&d.id).unwrap().cursor_usn, 900);
        assert_ne!(
            registry.get(&c.id).unwrap().journal_id,
            registry.get(&d.id).unwrap().journal_id
        );
    }

    #[test]
    fn an_advanced_cursor_produces_an_incremental_read() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0xC, r"C:\");
        registry.register(&c);
        registry.observe(&c.id, &journal(7, 0, 100));
        registry.advance(&c.id, 100, 0);

        let decision = registry.observe(&c.id, &journal(7, 0, 250));
        assert_eq!(
            decision,
            SyncDecision::Incremental {
                from_usn: 100,
                up_to_usn: 250
            }
        );
        assert_eq!(
            registry.get(&c.id).unwrap().status,
            VolumeSyncStatus::CatchingUp
        );
    }

    #[test]
    fn a_caught_up_volume_is_a_noop() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0xC, r"C:\");
        registry.register(&c);
        registry.observe(&c.id, &journal(7, 0, 100));
        registry.advance(&c.id, 100, 0);

        let decision = registry.observe(&c.id, &journal(7, 0, 100));
        assert!(decision.is_noop());
        assert_eq!(
            registry.get(&c.id).unwrap().status,
            VolumeSyncStatus::Healthy
        );
    }

    #[test]
    fn a_recreated_journal_forces_only_that_volume_to_rebuild() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0xC, r"C:\");
        let d = identity(r"\\?\Volume{d}\", 0xD, r"D:\");
        registry.register(&c);
        registry.register(&d);
        registry.observe(&c.id, &journal(1000, 0, 500));
        registry.observe(&d.id, &journal(2000, 0, 900));
        registry.advance(&c.id, 500, 0);
        registry.advance(&d.id, 900, 0);

        // C's journal is deleted and recreated with a new identifier.
        let decision = registry.observe(&c.id, &journal(9999, 0, 10));
        assert_eq!(
            decision,
            SyncDecision::Rebuild {
                reason: RebuildReason::JournalRecreated
            }
        );
        assert_eq!(registry.get(&c.id).unwrap().status, VolumeSyncStatus::Stale);

        // D is untouched and still incremental.
        assert_eq!(registry.get(&d.id).unwrap().journal_id, 2000);
        assert_eq!(
            registry.get(&d.id).unwrap().status,
            VolumeSyncStatus::Healthy
        );
        assert_eq!(
            registry.observe(&d.id, &journal(2000, 0, 1000)),
            SyncDecision::Incremental {
                from_usn: 900,
                up_to_usn: 1000
            }
        );
    }

    #[test]
    fn a_rollover_forces_a_rebuild_and_marks_the_volume_stale() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0xC, r"C:\");
        registry.register(&c);
        registry.observe(&c.id, &journal(7, 0, 100));
        registry.advance(&c.id, 100, 0);

        // The journal wrapped: everything below 5_000 is unreadable.
        let decision = registry.observe(&c.id, &journal(7, 5_000, 6_000));
        assert_eq!(
            decision,
            SyncDecision::Rebuild {
                reason: RebuildReason::CursorBeforeFirstUsn
            }
        );
        let state = registry.get(&c.id).unwrap();
        assert_eq!(state.status, VolumeSyncStatus::Stale);
        assert_eq!(state.cursor_usn, 5_000, "the cursor must not stay behind");
        assert!(state.last_error.is_some());
    }

    #[test]
    fn a_journal_that_moves_backwards_forces_a_rebuild() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0xC, r"C:\");
        registry.register(&c);
        registry.observe(&c.id, &journal(7, 0, 5_000));
        registry.advance(&c.id, 5_000, 0);

        let decision = registry.observe(&c.id, &journal(7, 0, 100));
        assert_eq!(
            decision,
            SyncDecision::Rebuild {
                reason: RebuildReason::CursorAheadOfJournal
            }
        );
        assert_eq!(registry.get(&c.id).unwrap().status, VolumeSyncStatus::Stale);
    }

    #[test]
    fn a_drive_letter_change_keeps_the_cursor() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{same}\", 0x42, r"C:\");
        registry.register(&c);
        registry.observe(&c.id, &journal(7, 0, 1_000));
        registry.advance(&c.id, 1_000, 5);

        // The same disk reappears as E:.
        let e = identity(r"\\?\Volume{same}\", 0x42, r"E:\");
        assert_eq!(
            registry.reconcile_identity(&e),
            IdentityReconciliation::SameVolume
        );
        assert!(registry.reconcile_identity(&e).cursor_is_reusable());

        let state = registry.get(&e.id).unwrap();
        assert_eq!(state.cursor_usn, 1_000, "the cursor must survive the move");
        assert_eq!(state.identity.mount_points, vec![r"E:\"]);
        assert_eq!(
            registry.len(),
            1,
            "a mount point change is not a new volume"
        );
    }

    #[test]
    fn an_unregistered_identity_is_reported_as_unknown() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0xC, r"C:\");
        registry.register(&c);
        let other = identity(r"\\?\Volume{other}\", 0x99, r"Z:\");
        assert_eq!(
            registry.reconcile_identity(&other),
            IdentityReconciliation::Unknown
        );
        assert!(!IdentityReconciliation::Unknown.cursor_is_reusable());
    }

    #[test]
    fn a_changed_serial_number_for_the_same_guid_is_flagged() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0x1111, r"C:\");
        registry.register(&c);
        let swapped = identity(r"\\?\Volume{c}\", 0x2222, r"C:\");
        assert_eq!(
            registry.reconcile_identity(&swapped),
            IdentityReconciliation::SerialChanged
        );
    }

    #[test]
    fn permission_denied_and_offline_are_recorded_without_losing_the_cursor() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0xC, r"C:\");
        registry.register(&c);
        registry.observe(&c.id, &journal(7, 0, 100));
        registry.advance(&c.id, 42, 3);

        registry.mark_permission_denied(&c.id, "access denied");
        let state = registry.get(&c.id).unwrap();
        assert_eq!(state.status, VolumeSyncStatus::PermissionDenied);
        assert_eq!(state.cursor_usn, 42);
        assert_eq!(state.last_error.as_deref(), Some("access denied"));

        registry.mark_offline(&c.id);
        assert_eq!(
            registry.get(&c.id).unwrap().status,
            VolumeSyncStatus::Offline
        );
        assert_eq!(registry.get(&c.id).unwrap().cursor_usn, 42);
    }

    #[test]
    fn degraded_reports_exactly_the_unhealthy_volumes() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0xC, r"C:\");
        let d = identity(r"\\?\Volume{d}\", 0xD, r"D:\");
        registry.register(&c);
        registry.register(&d);
        registry.observe(&c.id, &journal(1, 0, 10));
        registry.observe(&d.id, &journal(2, 0, 10));
        registry.advance(&c.id, 10, 0);
        registry.advance(&d.id, 10, 0);
        registry.mark_offline(&d.id);

        let degraded = registry.degraded();
        assert_eq!(degraded.len(), 1);
        assert_eq!(degraded[0].0, d.id);
        assert!(!registry.all_healthy());
    }

    #[test]
    fn invalidating_everything_keeps_identities_but_drops_cursors() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0xC, r"C:\");
        registry.register(&c);
        registry.observe(&c.id, &journal(7, 3_000, 4_000));
        registry.advance(&c.id, 4_000, 0);

        registry.invalidate_all(RebuildReason::Requested);
        let state = registry.get(&c.id).unwrap();
        assert_eq!(state.cursor_usn, 3_000);
        assert_eq!(state.status, VolumeSyncStatus::Stale);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn the_schema_version_is_part_of_the_persisted_shape() {
        let registry = JournalRegistry::new();
        assert!(registry.is_current());
        assert_eq!(registry.schema_version, JOURNAL_SCHEMA_VERSION);

        let mut old = registry.clone();
        old.schema_version = JOURNAL_SCHEMA_VERSION - 1;
        assert!(!old.is_current());
    }

    #[test]
    fn a_registry_round_trips_through_json() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0xC, r"C:\");
        registry.register(&c);
        registry.observe(&c.id, &journal(7, 0, 100));
        registry.advance(&c.id, 100, 12);

        let json = serde_json::to_string(&registry).unwrap();
        let restored: JournalRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(registry, restored);
    }

    #[test]
    fn a_registry_round_trips_through_messagepack() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0xC, r"C:\");
        registry.register(&c);
        registry.observe(&c.id, &journal(7, 0, 100));
        registry.advance(&c.id, 100, 12);

        let encoded = rmp_serde::to_vec_named(&registry).expect("encode");
        let restored: JournalRegistry = rmp_serde::from_slice(&encoded).expect("decode");
        assert_eq!(restored.schema_version, JOURNAL_SCHEMA_VERSION);
        assert_eq!(restored, registry);
        assert!(restored.get(&c.id).is_some());
    }

    #[test]
    fn lag_and_readability_describe_the_cursor() {
        let mut registry = JournalRegistry::new();
        let c = identity(r"\\?\Volume{c}\", 0xC, r"C:\");
        registry.register(&c);
        registry.observe(&c.id, &journal(7, 0, 1_000));
        registry.advance(&c.id, 400, 0);
        registry.observe(&c.id, &journal(7, 0, 1_000));
        let state = registry.get(&c.id).unwrap();
        assert_eq!(state.lag(), 600);
        assert!(state.cursor_is_readable());

        let mut rolled = state.clone();
        rolled.cursor_usn = 10;
        rolled.first_usn = 500;
        assert!(!rolled.cursor_is_readable());
    }

    #[test]
    fn every_status_has_a_stable_wire_name() {
        for status in [
            VolumeSyncStatus::Healthy,
            VolumeSyncStatus::CatchingUp,
            VolumeSyncStatus::Rebuilding,
            VolumeSyncStatus::Stale,
            VolumeSyncStatus::Offline,
            VolumeSyncStatus::PermissionDenied,
        ] {
            assert!(!status.as_str().is_empty());
            let json = serde_json::to_string(&status).unwrap();
            assert!(json.contains(status.as_str()), "{json}");
        }
        assert!(VolumeSyncStatus::Healthy.is_current());
        assert!(!VolumeSyncStatus::Stale.is_current());
        assert!(VolumeSyncStatus::Stale.is_degraded());
    }
}
