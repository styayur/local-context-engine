//! A very small read-only registry wrapper.
//!
//! Only three operations are needed for application discovery: open a key,
//! list its subkeys, and read a string value. Everything is read-only, and
//! every failure is a `None` rather than an error — a registry key the current
//! user cannot read simply contributes no applications.

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE,
};

/// `KEY_QUERY_VALUE | KEY_ENUMERATE_SUB_KEYS | KEY_NOTIFY | STANDARD_RIGHTS_READ`.
pub(crate) const KEY_READ: u32 = 0x2_0019;
/// Read the 64-bit view of the registry.
pub(crate) const KEY_WOW64_64KEY: u32 = 0x0100;
/// Read the 32-bit (WOW64) view of the registry.
pub(crate) const KEY_WOW64_32KEY: u32 = 0x0200;

const ERROR_SUCCESS: u32 = 0;
const ERROR_NO_MORE_ITEMS: u32 = 259;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;

/// Which predefined hive to read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegRoot {
    /// `HKEY_LOCAL_MACHINE`.
    LocalMachine,
    /// `HKEY_CURRENT_USER`.
    CurrentUser,
}

impl RegRoot {
    const fn raw(self) -> HKEY {
        match self {
            RegRoot::LocalMachine => HKEY_LOCAL_MACHINE,
            RegRoot::CurrentUser => HKEY_CURRENT_USER,
        }
    }
}

/// Open a subkey, returning `None` when it does not exist or cannot be read.
#[must_use]
pub(crate) fn open(root: RegRoot, path: &str, flags: u32) -> Option<HKEY> {
    let wide = to_wide(path);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `wide` is NUL terminated and `key` is a valid out-parameter.
    let status = unsafe { RegOpenKeyExW(root.raw(), wide.as_ptr(), 0, flags, &mut key) };
    (status == ERROR_SUCCESS && !key.is_null()).then_some(key)
}

/// Close a key previously returned by [`open`].
pub(crate) fn close(key: HKEY) {
    if key.is_null() {
        return;
    }
    // SAFETY: `key` came from RegOpenKeyExW and is not used afterwards.
    unsafe { RegCloseKey(key) };
}

/// The names of every immediate subkey.
#[must_use]
pub(crate) fn enum_subkeys(key: HKEY) -> Vec<String> {
    let mut names = Vec::new();
    let mut buffer = vec![0u16; 512];
    for index in 0.. {
        let mut length = u32::try_from(buffer.len()).unwrap_or(0);
        // SAFETY: the buffer is valid for `length` UTF-16 code units and every
        // optional out-parameter is passed as null.
        let status = unsafe {
            RegEnumKeyExW(
                key,
                index,
                buffer.as_mut_ptr(),
                &mut length,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut::<FILETIME>(),
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status != ERROR_SUCCESS {
            // ENUM truncation or a buffer that is too small: widen and retry
            // this index rather than giving up on the whole key.
            if status == 234 && buffer.len() < 4_096 {
                buffer.resize(buffer.len() * 2, 0);
                continue;
            }
            break;
        }
        names.push(String::from_utf16_lossy(&buffer[..length as usize]));
    }
    names
}

/// Read a string value. `None` reads the key's default value.
#[must_use]
pub(crate) fn query_string(key: HKEY, value_name: Option<&str>) -> Option<String> {
    let wide_name = value_name.map(to_wide);
    let name_pointer = wide_name
        .as_ref()
        .map_or(std::ptr::null(), |name| name.as_ptr());

    let mut kind = 0u32;
    let mut size = 0u32;
    // SAFETY: a null buffer with size zero is the documented size probe.
    let status = unsafe {
        RegQueryValueExW(
            key,
            name_pointer,
            std::ptr::null_mut(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS || size == 0 || size > 1024 * 1024 {
        return None;
    }

    let mut buffer = vec![0u8; size as usize];
    // SAFETY: the buffer is exactly `size` bytes as reported by the probe.
    let status = unsafe {
        RegQueryValueExW(
            key,
            name_pointer,
            std::ptr::null_mut(),
            &mut kind,
            buffer.as_mut_ptr(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }

    // The payload is UTF-16 including the trailing NUL.
    let units: Vec<u16> = buffer
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    let value = String::from_utf16_lossy(&units);
    match kind {
        REG_SZ => Some(value),
        REG_EXPAND_SZ => Some(expand_environment(&value)),
        _ => None,
    }
}

fn expand_environment(value: &str) -> String {
    let wide = to_wide(value);
    // SAFETY: a null destination with size zero returns the required length.
    let needed = unsafe { ExpandEnvironmentStringsW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    if needed == 0 || needed > 32_768 {
        return value.to_string();
    }
    let mut buffer = vec![0u16; needed as usize];
    // SAFETY: the buffer is valid for `needed` UTF-16 code units.
    let written = unsafe { ExpandEnvironmentStringsW(wide.as_ptr(), buffer.as_mut_ptr(), needed) };
    if written == 0 {
        return value.to_string();
    }
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
    fn a_well_known_key_can_be_opened_and_closed() {
        let key = open(
            RegRoot::LocalMachine,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths",
            KEY_READ | KEY_WOW64_64KEY,
        );
        if let Some(key) = key {
            let _ = enum_subkeys(key);
            close(key);
        }
    }

    #[test]
    fn a_missing_key_returns_none() {
        assert!(open(
            RegRoot::CurrentUser,
            r"SOFTWARE\LocalContextEngine\DoesNotExist",
            KEY_READ,
        )
        .is_none());
    }

    #[test]
    fn the_windows_nt_version_value_is_readable() {
        let key = open(
            RegRoot::LocalMachine,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            KEY_READ | KEY_WOW64_64KEY,
        );
        if let Some(key) = key {
            let product = query_string(key, Some("ProductName"));
            close(key);
            if let Some(product) = product {
                assert!(
                    product.to_lowercase().contains("windows"),
                    "unexpected product name: {product}"
                );
            }
        }
    }

    #[test]
    fn app_paths_enumeration_does_not_panic() {
        if let Some(key) = open(
            RegRoot::LocalMachine,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths",
            KEY_READ | KEY_WOW64_64KEY,
        ) {
            let subkeys = enum_subkeys(key);
            close(key);
            for name in subkeys {
                assert!(!name.is_empty());
            }
        }
    }
}
