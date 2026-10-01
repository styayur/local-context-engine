//! Volume discovery.

use search_core::{LceError, PlatformError};
use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};

const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;

/// One mounted volume the indexer can consider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeSpec {
    /// Root path, always with a trailing separator, for example `C:\`.
    pub root: String,
    /// Drive letter.
    pub drive: char,
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
    /// Label used in the UI and in cache file names.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}:", self.drive)
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

    Some(VolumeSpec {
        root: root.to_string(),
        drive,
        file_system,
        is_ntfs,
        total_bytes: (space_ok != 0).then_some(total),
        free_bytes: (space_ok != 0).then_some(free),
        needs_elevation: is_ntfs,
    })
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
    fn drive_helpers_agree_with_search_core() {
        assert_eq!(root_for_drive('d'), r"D:\");
        assert_eq!(volume_of_path(r"c:\Users"), Some('C'));
        assert_eq!(volume_of_path(r"\\server\share"), None);
    }
}
