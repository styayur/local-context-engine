//! Volume discovery.

use std::fmt;

use serde::{Deserialize, Serialize};

use search_core::{LceError, PlatformError};
use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    GetVolumeNameForVolumeMountPointW,
};

/// A stable identifier for a volume.
///
/// **Never a drive letter.** `C:` is a mount point: it can be reassigned, a
/// volume can be mounted into a folder, and the same letter can point at a
/// different disk after a reboot. The identifier here is the volume GUID path
/// (`\\?\Volume{...}\`), or, when the system will not hand one over, the
/// volume serial number.
///
/// The value is stored normalised — lower-case, without a trailing backslash —
/// so that a cache written on one boot is recognised on the next.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VolumeId(String);

impl VolumeId {
    /// Build an identifier from an already normalised string.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(normalise_id(&raw.into()))
    }

    /// The identifier from a volume serial number, used when no GUID exists.
    #[must_use]
    pub fn from_serial(serial: u32) -> Self {
        Self(format!("serial:{serial:08x}"))
    }

    /// The stored, normalised string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this identifier came from a volume GUID rather than a serial.
    #[must_use]
    pub fn is_guid(&self) -> bool {
        self.0.starts_with("\\\\?\\volume{")
    }

    /// A short label for the UI and for log lines.
    #[must_use]
    pub fn short(&self) -> String {
        if self.is_guid() {
            // `\\?\volume{1a2b3c4d-...}` -> `Volume{1a2b3c4d-`
            let inner = self.0.trim_start_matches("\\\\?\\").trim_end_matches('\\');
            let rest = inner.strip_prefix("volume").unwrap_or(inner);
            format!("Volume{rest}").chars().take(17).collect()
        } else {
            self.0.clone()
        }
    }
}

impl fmt::Display for VolumeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn normalise_id(raw: &str) -> String {
    raw.trim()
        .trim_end_matches(['\\', '/'])
        .to_lowercase()
        .to_string()
}

/// Everything needed to recognise a volume across reboots and re-lettings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeIdentity {
    /// Stable identifier, see [`VolumeId`].
    pub id: VolumeId,
    /// The NTFS/ReFS serial number, a secondary identity signal.
    pub serial_number: u32,
    /// File system name, for example `NTFS`.
    pub file_system: String,
    /// Every mount point currently associated with the volume, for example
    /// `C:\`. Sorted and deduplicated.
    pub mount_points: Vec<String>,
}

impl VolumeIdentity {
    /// Build an identity.
    #[must_use]
    pub fn new(id: VolumeId, serial_number: u32, file_system: impl Into<String>) -> Self {
        Self {
            id,
            serial_number,
            file_system: file_system.into(),
            mount_points: Vec::new(),
        }
    }

    /// Add a mount point, keeping the list sorted and unique.
    pub fn add_mount_point(&mut self, mount_point: impl Into<String>) {
        let mount_point = mount_point.into();
        if !self.mount_points.contains(&mount_point) {
            self.mount_points.push(mount_point);
            self.mount_points.sort();
        }
    }

    /// Whether the identity matches a volume that is now mounted elsewhere but
    /// is still the same disk.
    ///
    /// This is the check that lets a drive-letter change keep its index instead
    /// of forcing a rebuild.
    #[must_use]
    pub fn same_volume_as(&self, other: &VolumeIdentity) -> bool {
        self.id == other.id
            || (self.serial_number != 0 && self.serial_number == other.serial_number)
    }

    /// A human readable label: the first mount point, or the short id.
    #[must_use]
    pub fn label(&self) -> String {
        match self.mount_points.first() {
            Some(mount_point) => mount_point.clone(),
            None => self.id.short(),
        }
    }
}

const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;

/// One mounted volume the indexer can consider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeSpec {
    /// Root path, always with a trailing separator, for example `C:\`.
    pub root: String,
    /// Drive letter this volume is currently mounted on.
    ///
    /// This is a *mount point*, not an identity: it may change between boots.
    /// Persist [`VolumeSpec::identity`] instead.
    pub drive: char,
    /// Stable identity of the underlying volume.
    pub identity: VolumeIdentity,
    /// File system name, for example `NTFS`.
    pub file_system: String,
    /// Whether the volume uses NTFS, which is what the MFT backend needs.
    pub is_ntfs: bool,
    /// Total size in bytes, when it can be read.
    pub total_bytes: Option<u64>,
    /// Free space in bytes, when it can be read.
    pub free_bytes: Option<u64>,
    /// Whether the volume needs administrator rights to enumerate the MFT.
    pub needs_elevation: bool,
}

impl VolumeSpec {
    /// Build a volume description.
    #[must_use]
    pub fn new(
        root: impl Into<String>,
        drive: char,
        identity: VolumeIdentity,
        file_system: impl Into<String>,
        is_ntfs: bool,
    ) -> Self {
        let file_system = file_system.into();
        Self {
            root: root.into(),
            drive,
            identity,
            file_system,
            is_ntfs,
            total_bytes: None,
            free_bytes: None,
            needs_elevation: is_ntfs,
        }
    }

    /// A volume description with a synthetic identity, for tests and for cache
    /// schemas that predate volume GUIDs.
    #[must_use]
    pub fn synthetic(root: impl Into<String>, drive: char, is_ntfs: bool) -> Self {
        let root = root.into();
        let mut identity = VolumeIdentity::new(
            VolumeId::from_serial(0),
            0,
            if is_ntfs { "NTFS" } else { "FAT" },
        );
        identity.add_mount_point(root.clone());
        Self::new(
            root,
            drive,
            identity,
            if is_ntfs { "NTFS" } else { "FAT" },
            is_ntfs,
        )
    }

    /// The stable identifier to persist.
    #[must_use]
    pub fn id(&self) -> &VolumeId {
        &self.identity.id
    }

    /// Label used in the UI and in cache file names.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}:", self.drive)
    }

    /// A label that survives a drive-letter change.
    #[must_use]
    pub fn stable_label(&self) -> String {
        format!("{} ({})", self.label(), self.identity.id.short())
    }
}

/// Enumerate local fixed and remote volumes.
///
/// Removable and optical drives are skipped: they are usually absent, and
/// indexing them would produce a stale index that never invalidates cleanly.
#[must_use]
pub fn list_volumes() -> Vec<VolumeSpec> {
    // SAFETY: GetLogicalDrives takes no arguments and cannot fail unsafely.
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return Vec::new();
    }

    let mut volumes = Vec::new();
    for index in 0..26u32 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let Some(letter) = char::from_u32(u32::from(b'A') + index) else {
            continue;
        };
        let root = format!("{letter}:\\");
        // SAFETY: `root` is a NUL terminated UTF-16 buffer for the call.
        let wide_root = to_wide(&root);
        let drive_type = unsafe { GetDriveTypeW(wide_root.as_ptr()) };
        if drive_type != DRIVE_FIXED && drive_type != DRIVE_REMOTE {
            continue;
        }
        if let Some(volume) = probe(&root, letter) {
            volumes.push(volume);
        }
    }
    volumes
}

fn probe(root: &str, drive: char) -> Option<VolumeSpec> {
    let wide_root = to_wide(root);

    let mut volume_name = vec![0u16; 261];
    let mut file_system = vec![0u16; 261];
    let mut serial = 0u32;
    let mut max_component = 0u32;
    let mut flags = 0u32;

    // SAFETY: every buffer is valid for the length passed alongside it.
    let ok = unsafe {
        GetVolumeInformationW(
            wide_root.as_ptr(),
            volume_name.as_mut_ptr(),
            u32::try_from(volume_name.len()).ok()?,
            &mut serial,
            &mut max_component,
            &mut flags,
            file_system.as_mut_ptr(),
            u32::try_from(file_system.len()).ok()?,
        )
    };
    if ok == 0 {
        return None;
    }

    let file_system = nul_terminated(&file_system);
    let is_ntfs = file_system.eq_ignore_ascii_case("NTFS");

    let mut free_to_caller = 0u64;
    let mut total = 0u64;
    let mut free = 0u64;
    // SAFETY: all three pointers are valid out-parameters.
    let space_ok = unsafe {
        GetDiskFreeSpaceExW(
            wide_root.as_ptr(),
            &mut free_to_caller,
            &mut total,
            &mut free,
        )
    };

    let mut identity = resolve_identity(root, serial, &file_system);
    identity.add_mount_point(root);

    Some(VolumeSpec {
        root: root.to_string(),
        drive,
        identity,
        file_system,
        is_ntfs,
        total_bytes: (space_ok != 0).then_some(total),
        free_bytes: (space_ok != 0).then_some(free),
        needs_elevation: is_ntfs,
    })
}

/// Resolve the stable identity of the volume mounted at `root`.
///
/// `GetVolumeNameForVolumeMountPointW` returns the volume GUID path, which is
/// the only identifier that survives a drive-letter change. When the system
/// refuses (network shares, some virtual volumes) the serial number is used
/// instead, and failing that the mount point is the last resort.
#[must_use]
pub fn resolve_identity(root: &str, serial: u32, file_system: &str) -> VolumeIdentity {
    let id = volume_guid_for_mount_point(root)
        .map(VolumeId::new)
        .unwrap_or_else(|| VolumeId::from_serial(serial));
    VolumeIdentity::new(id, serial, file_system)
}

/// The volume GUID path for a mount point, for example `C:\`.
#[must_use]
pub fn volume_guid_for_mount_point(root: &str) -> Option<String> {
    let wide_root = to_wide(root);
    let mut buffer = vec![0u16; 260];
    // SAFETY: the buffer is valid for the length passed in.
    let ok = unsafe {
        GetVolumeNameForVolumeMountPointW(
            wide_root.as_ptr(),
            buffer.as_mut_ptr(),
            u32::try_from(buffer.len()).ok()?,
        )
    };
    if ok == 0 {
        return None;
    }
    let name = nul_terminated(&buffer);
    (!name.is_empty()).then_some(name)
}

/// The stable identity of the volume that currently holds `drive`.
#[must_use]
pub fn identity_for_drive(drive: char) -> Option<VolumeIdentity> {
    let root = root_for_drive(drive);
    let wide_root = to_wide(&root);
    let mut file_system = vec![0u16; 261];
    let mut serial = 0u32;
    let mut max_component = 0u32;
    let mut flags = 0u32;
    // SAFETY: every buffer is valid for the length passed alongside it.
    let ok = unsafe {
        GetVolumeInformationW(
            wide_root.as_ptr(),
            std::ptr::null_mut(),
            0,
            &mut serial,
            &mut max_component,
            &mut flags,
            file_system.as_mut_ptr(),
            u32::try_from(file_system.len()).ok()?,
        )
    };
    if ok == 0 {
        return None;
    }
    let file_system = nul_terminated(&file_system);
    let mut identity = resolve_identity(&root, serial, &file_system);
    identity.add_mount_point(root);
    Some(identity)
}

/// The volume a fully qualified path belongs to, when it has a drive letter.
#[must_use]
pub fn volume_of_path(path: &str) -> Option<char> {
    search_core::drive_of(path)
}

/// Turn a drive letter into the canonical root path.
#[must_use]
pub fn root_for_drive(drive: char) -> String {
    format!("{}:\\", drive.to_ascii_uppercase())
}

/// Read the volumes and describe what the indexer will do.
pub fn describe_volumes() -> Result<Vec<VolumeSpec>, LceError> {
    let volumes = list_volumes();
    if volumes.is_empty() {
        return Err(LceError::Platform(PlatformError::NotFound {
            resource: "any fixed volume".into(),
        }));
    }
    Ok(volumes)
}

fn nul_terminated(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_least_one_volume_is_present_on_a_windows_machine() {
        let volumes = list_volumes();
        assert!(
            !volumes.is_empty(),
            "a Windows installation always has a system volume"
        );
        assert!(volumes.iter().any(|volume| volume.drive == 'C'));
    }

    #[test]
    fn volume_roots_end_with_a_separator() {
        for volume in list_volumes() {
            assert!(volume.root.ends_with('\\'), "{}", volume.root);
            assert_eq!(volume.root.len(), 3);
        }
    }

    #[test]
    fn ntfs_detection_is_reported() {
        let volumes = list_volumes();
        if let Some(system) = volumes.iter().find(|volume| volume.drive == 'C') {
            assert!(!system.file_system.is_empty());
            assert_eq!(
                system.is_ntfs,
                system.file_system.eq_ignore_ascii_case("NTFS")
            );
        }
    }

    #[test]
    fn a_volume_id_is_normalised_and_case_insensitive() {
        let a = VolumeId::new(r"\\?\Volume{1A2B3C4D-0000-0000-0000-000000000000}\");
        let b = VolumeId::new(r"\\?\volume{1a2b3c4d-0000-0000-0000-000000000000}");
        assert_eq!(a, b, "the same volume must hash and compare equal");
        assert!(a.is_guid());
        assert!(!VolumeId::from_serial(0x1234_5678).is_guid());
    }

    #[test]
    fn serial_ids_are_zero_padded_so_they_sort_stably() {
        assert_eq!(VolumeId::from_serial(0xff).as_str(), "serial:000000ff");
    }

    #[test]
    fn short_labels_stay_readable() {
        let id = VolumeId::new(r"\\?\Volume{1a2b3c4d-5e6f-7890-abcd-ef0123456789}\");
        assert_eq!(id.short(), "Volume{1a2b3c4d-5");
        assert_eq!(VolumeId::from_serial(1).short(), "serial:00000001");
    }

    #[test]
    fn identity_survives_a_drive_letter_change() {
        // Same disk, different mount point: the identity must still match.
        let mut first =
            VolumeIdentity::new(VolumeId::new(r"\\?\Volume{aaaa}\"), 0xDEAD_BEEF, "NTFS");
        first.add_mount_point(r"C:\");
        let mut second = first.clone();
        second.mount_points.clear();
        second.add_mount_point(r"D:\");

        assert!(first.same_volume_as(&second));
        assert_eq!(first.label(), r"C:\");
        assert_eq!(second.label(), r"D:\");
    }

    #[test]
    fn different_serials_are_different_volumes() {
        let first = VolumeIdentity::new(VolumeId::new("serial:00000001"), 1, "NTFS");
        let second = VolumeIdentity::new(VolumeId::new("serial:00000002"), 2, "NTFS");
        assert!(!first.same_volume_as(&second));
    }

    #[test]
    fn mount_points_stay_sorted_and_unique() {
        let mut identity = VolumeIdentity::new(VolumeId::from_serial(1), 1, "NTFS");
        identity.add_mount_point(r"D:\");
        identity.add_mount_point(r"C:\");
        identity.add_mount_point(r"D:\");
        assert_eq!(identity.mount_points, vec![r"C:\", r"D:\"]);
    }

    #[test]
    fn the_system_volume_has_a_resolvable_identity() {
        let volumes = list_volumes();
        let Some(system) = volumes.iter().find(|volume| volume.drive == 'C') else {
            return;
        };
        assert!(!system.identity.id.as_str().is_empty());
        assert!(
            system.identity.id.is_guid() || system.identity.serial_number != 0,
            "the system volume must have a GUID or a serial number"
        );
        assert!(system.identity.mount_points.contains(&system.root));
    }

    #[test]
    fn a_synthetic_volume_is_labelled_like_the_real_thing() {
        let volume = VolumeSpec::synthetic(r"X:\", 'X', true);
        assert_eq!(volume.label(), "X:");
        assert_eq!(volume.root, r"X:\");
        assert!(volume.identity.mount_points.contains(&r"X:\".to_string()));
    }

    #[test]
    fn drive_helpers_agree_with_search_core() {
        assert_eq!(root_for_drive('d'), r"D:\");
        assert_eq!(volume_of_path(r"c:\Users"), Some('C'));
        assert_eq!(volume_of_path(r"\\server\share"), None);
    }
}
