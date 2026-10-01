//! Raw top-level window enumeration.

use std::collections::HashMap;

use search_core::{LceError, PlatformError, WindowEntry};
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible,
};

const MAX_CLASS_NAME: usize = 256;
const MAX_IMAGE_PATH: usize = 32_768;

/// Enumerate top-level windows that have a title.
///
/// Untitled windows are skipped: they are almost always hidden message-only
/// windows and would only add noise to the result list.
pub fn list_windows() -> Result<Vec<WindowEntry>, LceError> {
    let mut handles: Vec<HWND> = Vec::with_capacity(256);
    let pointer: *mut Vec<HWND> = &mut handles;
    // SAFETY: `pointer` stays valid for the duration of the call, and the
    // callback only writes to that Vec.
    let ok = unsafe { EnumWindows(Some(collect_window), pointer as LPARAM) };
    if ok == 0 {
        return Err(LceError::Platform(PlatformError::WindowsApi {
            call: "EnumWindows".into(),
            code: last_error(),
        }));
    }

    let mut process_names: HashMap<u32, Option<String>> = HashMap::new();
    let mut entries = Vec::with_capacity(handles.len());

    for hwnd in handles {
        let Some(title) = window_title(hwnd) else {
            continue;
        };
        if title.trim().is_empty() {
            continue;
        }

        let mut pid = 0u32;
        // SAFETY: `pid` is a valid out-parameter.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };

        let process_name = process_names
            .entry(pid)
            .or_insert_with(|| process_image_name(pid))
            .clone();

        // SAFETY: IsWindowVisible only reads window state.
        let visible = unsafe { IsWindowVisible(hwnd) } != 0;

        entries.push(WindowEntry {
            hwnd: hwnd as i64,
            title,
            pid,
            process_name,
            visible,
            class_name: window_class(hwnd),
        });
    }

    Ok(entries)
}

/// SAFETY: called by `EnumWindows` with a `LPARAM` that this module set to a
/// valid `*mut Vec<HWND>`.
unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> i32 {
    if lparam == 0 {
        return 0;
    }
    // SAFETY: the contract above guarantees `lparam` is a live `*mut Vec<HWND>`
    // that outlives the enumeration.
    let list = unsafe { &mut *(lparam as *mut Vec<HWND>) };
    list.push(hwnd);
    1
}

fn window_title(hwnd: HWND) -> Option<String> {
    // SAFETY: a pure query of the window's caption length.
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return None;
    }
    let mut buffer = vec![0u16; usize::try_from(length).ok()? + 1];
    // SAFETY: the buffer is valid for `buffer.len()` UTF-16 code units.
    let written = unsafe {
        GetWindowTextW(
            hwnd,
            buffer.as_mut_ptr(),
            i32::try_from(buffer.len()).unwrap_or(i32::MAX),
        )
    };
    if written <= 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..written as usize]))
}

fn window_class(hwnd: HWND) -> Option<String> {
    let mut buffer = vec![0u16; MAX_CLASS_NAME];
    // SAFETY: the buffer is valid for `buffer.len()` UTF-16 code units.
    let written = unsafe {
        GetClassNameW(
            hwnd,
            buffer.as_mut_ptr(),
            i32::try_from(buffer.len()).unwrap_or(i32::MAX),
        )
    };
    if written <= 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..written as usize]))
}

fn process_image_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    // SAFETY: a documented access mask; a null handle is handled below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }

    let mut buffer = vec![0u16; MAX_IMAGE_PATH];
    let mut length = u32::try_from(buffer.len()).unwrap_or(0);
    // SAFETY: the buffer is valid for `length` UTF-16 code units.
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) };
    // SAFETY: `handle` came from OpenProcess and is not used again.
    unsafe { CloseHandle(handle) };
    if ok == 0 || length == 0 {
        return None;
    }

    let path = String::from_utf16_lossy(&buffer[..length as usize]);
    Some(search_core::file_name_of(&path))
}

fn last_error() -> i64 {
    // SAFETY: GetLastError is always safe to call.
    i64::from(unsafe { windows_sys::Win32::Foundation::GetLastError() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_snapshot_is_produced_without_panicking() {
        // A headless CI runner may legitimately have zero titled windows, so
        // only the "did not fail" property is asserted.
        let windows = list_windows().expect("window enumeration must not fail");
        for window in &windows {
            assert!(!window.title.trim().is_empty());
            assert!(window.hwnd != 0);
        }
    }

    #[test]
    fn every_reported_window_carries_a_process_id_and_a_handle() {
        let windows = list_windows().expect("window enumeration must not fail");
        for window in windows {
            assert_ne!(window.hwnd, 0, "a window handle of zero is not a window");
            assert!(window.pid != 0, "every titled window belongs to a process");
            if let Some(class) = &window.class_name {
                assert!(!class.is_empty(), "class names are never empty strings");
            }
        }
    }
}
