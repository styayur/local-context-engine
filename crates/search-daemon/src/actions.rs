//! User-initiated actions on a search result.
//!
//! Everything here is an *explicit* user action: nothing is ever executed as a
//! consequence of a query. The only destructive operation, ending a process,
//! additionally requires `confirmed = true`, which the UI only sets after the
//! user accepts a confirmation dialog.

use search_core::{EntityType, LceError, LocalEntity, PermissionError, PlatformError};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const SE_ERR_ACCESSDENIED: isize = 5;

/// How an entity should be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTarget {
    /// Hand the path to its registered handler (a document, an executable).
    Default,
    /// Hand the path to the file manager, selecting the item where possible.
    Directory,
}

/// Open an entity with the shell.
pub fn open(entity: &LocalEntity, target: OpenTarget) -> Result<(), LceError> {
    let path = entity.path().ok_or_else(|| {
        LceError::Platform(PlatformError::NotFound {
            resource: format!("a filesystem location for `{}`", entity.name()),
        })
    })?;

    match target {
        OpenTarget::Default => shell_execute("open", path, None).map(|_| ()),
        OpenTarget::Directory => reveal(path),
    }
}

/// Open the entity's containing folder, selecting the item when it has one.
pub fn reveal(path: &str) -> Result<(), LceError> {
    let trimmed = path.trim().trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        return Err(LceError::Platform(PlatformError::NotFound {
            resource: "a path to reveal".into(),
        }));
    }
    // `explorer.exe /select,<path>` needs the raw path and no quoting of the
    // whole argument, which is why this goes through the shell verb rather
    // than a command line string built by hand.
    let arguments = format!("/select,\"{trimmed}\"");
    let result = shell_execute("open", "explorer.exe", Some(&arguments))?;
    let _ = result;
    Ok(())
}

/// Launch an application by its shell target.
pub fn launch(target: &str, arguments: Option<&str>) -> Result<(), LceError> {
    shell_execute("open", target, arguments).map(|_| ())
}

/// End a process.
///
/// `confirmed` must be `true`; the UI sets it only after the user accepts a
/// confirmation dialog. Anything else is refused with
/// [`PermissionError::AccessDenied`].
pub fn terminate_process(pid: u32, confirmed: bool) -> Result<(), LceError> {
    if !confirmed {
        return Err(LceError::Permission(PermissionError::AccessDenied {
            action: format!("ending process {pid} without confirmation"),
        }));
    }

    // SAFETY: OpenProcess with a documented access mask; the handle is checked
    // before use and closed on every path.
    let handle: HANDLE = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return Err(LceError::Permission(PermissionError::AccessDenied {
            action: format!("ending process {pid}"),
        }));
    }

    // SAFETY: `handle` came from OpenProcess with PROCESS_TERMINATE and is not
    // used after this call.
    let ok = unsafe { TerminateProcess(handle, 1) };
    // SAFETY: `handle` is closed exactly once.
    unsafe { CloseHandle(handle) };

    if ok == 0 {
        return Err(LceError::Platform(PlatformError::WindowsApi {
            call: "TerminateProcess".into(),
            code: i64::from(last_error()),
        }));
    }
    Ok(())
}

/// Whether an action is destructive and therefore needs confirmation.
#[must_use]
pub const fn requires_confirmation(entity_type: EntityType) -> bool {
    matches!(entity_type, EntityType::Process)
}

fn shell_execute(verb: &str, target: &str, parameters: Option<&str>) -> Result<isize, LceError> {
    let verb = to_wide(verb);
    let target = to_wide(target);
    let parameters = parameters.map(to_wide);

    // SAFETY: every string is a NUL terminated UTF-16 buffer that outlives the
    // call, and the unused arguments are null.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            parameters
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };

    let code = result as isize;
    if code <= 32 {
        return Err(if code == SE_ERR_ACCESSDENIED {
            LceError::Permission(PermissionError::AccessDenied {
                action: format!("opening `{}`", String::from_utf16_lossy(&target[..])),
            })
        } else {
            LceError::Platform(PlatformError::WindowsApi {
                call: "ShellExecuteW".into(),
                code: code as i64,
            })
        });
    }
    Ok(code)
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error() -> u32 {
    // SAFETY: GetLastError is always safe to call.
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use search_core::{FileEntry, ProcessEntry};

    fn file(path: &str) -> LocalEntity {
        LocalEntity::File(FileEntry {
            path: path.to_string(),
            name: search_core::file_name_of(path),
            extension: None,
            size: 0,
            modified: None,
            created: None,
            drive: path.chars().next(),
            file_id: None,
        })
    }

    #[test]
    fn terminating_without_confirmation_is_refused() {
        let error = terminate_process(4, false).unwrap_err();
        assert_eq!(error.code(), "access-denied");
        assert!(error.to_string().contains('4'));
    }

    #[test]
    fn only_processes_are_destructive() {
        assert!(requires_confirmation(EntityType::Process));
        assert!(!requires_confirmation(EntityType::File));
        assert!(!requires_confirmation(EntityType::Application));
        assert!(!requires_confirmation(EntityType::Service));
    }

    #[test]
    fn revealing_an_empty_path_is_rejected() {
        let error = reveal("   ").unwrap_err();
        assert_eq!(error.code(), "not-found");
    }

    #[test]
    fn opening_a_window_result_has_no_path_to_open() {
        let entity = LocalEntity::Window(search_core::WindowEntry {
            hwnd: 1,
            title: "Untitled".into(),
            pid: 1,
            process_name: None,
            visible: true,
            class_name: None,
        });
        let error = open(&entity, OpenTarget::Default).unwrap_err();
        assert_eq!(error.code(), "not-found");
    }

    #[test]
    fn a_process_entity_reports_its_executable_path() {
        let entity = LocalEntity::Process(ProcessEntry {
            pid: 7,
            name: "node.exe".into(),
            exe_path: Some(r"C:\Program Files\nodejs\node.exe".into()),
            parent_pid: None,
            memory_bytes: 0,
            start_time: None,
            username: None,
            thread_count: 1,
            session_id: None,
        });
        assert_eq!(entity.path(), Some(r"C:\Program Files\nodejs\node.exe"));
    }

    #[test]
    fn file_entities_expose_their_path() {
        assert_eq!(file(r"C:\notes.txt").path(), Some(r"C:\notes.txt"));
    }
}
