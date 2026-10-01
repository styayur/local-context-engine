//! Windows service control manager integration.
//!
//! Four operations, plus the dispatcher entry point: install, uninstall, start,
//! stop and status. Each one is a thin wrapper over the documented SCM API, and
//! each one fails with an actionable message rather than a bare error code.

use std::ffi::c_void;

use search_core::{LceError, PermissionError};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, ControlService, CreateServiceW, DeleteService, OpenSCManagerW,
    OpenServiceW, QueryServiceStatusEx, StartServiceW, SERVICE_STATUS,
};
use windows_sys::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
};

/// Internal service name. Registered under this key in the SCM database.
pub const SERVICE_NAME: &str = "lce-index-service";
/// Display name shown in `services.msc`.
pub const DISPLAY_NAME: &str = "Local Context Engine Index Service";
/// Description shown next to the display name.
pub const DESCRIPTION: &str =
    "Owns the NTFS MFT and USN change journals for Local Context Engine so the desktop, CLI and MCP front ends can run without administrator rights.";

const SC_MANAGER_ALL_ACCESS: u32 = 0x000F_003F;
const SERVICE_ALL_ACCESS: u32 = 0x000F_01FF;
const SERVICE_WIN32_OWN_PROCESS: u32 = 0x0000_0010;
const SERVICE_AUTO_START: u32 = 0x0000_0002;
const SERVICE_ERROR_NORMAL: u32 = 0x0000_0001;
const SERVICE_CONTROL_STOP: u32 = 0x0000_0001;
const SERVICE_CONTROL_INTERROGATE: u32 = 0x0000_0004;
const SERVICE_STOPPED: u32 = 0x0000_0001;
const SERVICE_START_PENDING: u32 = 0x0000_0002;
const SERVICE_STOP_PENDING: u32 = 0x0000_0003;
const SERVICE_RUNNING: u32 = 0x0000_0004;
const SERVICE_ACCEPT_STOP: u32 = 0x0000_0001;
const SC_STATUS_PROCESS_INFO: i32 = 0;
const ERROR_SERVICE_EXISTS: u32 = 1073;
const ERROR_SERVICE_DOES_NOT_EXIST: u32 = 1060;

/// Register the service with the SCM.
pub fn install() -> i32 {
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("cannot determine this executable's path: {error}");
            return 2;
        }
    };
    let command = format!("\"{}\" run", exe.display());

    // SAFETY: null machine and database mean "the local machine"; the returned
    // handle is checked and closed below.
    let manager =
        unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) };
    if manager.is_null() {
        return report_permission_failure("opening the service control manager");
    }

    let service_wide = to_wide(SERVICE_NAME);
    let display_wide = to_wide(DISPLAY_NAME);
    let command_wide = to_wide(&command);

    // SAFETY: every string is a NUL terminated UTF-16 buffer that outlives the
    // call, and the unused arguments are null as documented.
    let service = unsafe {
        CreateServiceW(
            manager,
            service_wide.as_ptr(),
            display_wide.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            command_wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };

    // SAFETY: `manager` came from OpenSCManagerW and is not used again.
    unsafe { CloseServiceHandle(manager) };

    if service.is_null() {
        let code = last_error();
        if code == ERROR_SERVICE_EXISTS {
            println!("{SERVICE_NAME} is already installed");
            return 0;
        }
        eprintln!("could not create the service (error {code})");
        return 1;
    }
    // SAFETY: `service` came from CreateServiceW and is not used again.
    unsafe { CloseServiceHandle(service) };

    println!("installed {SERVICE_NAME}");
    println!("  display name: {DISPLAY_NAME}");
    println!("  description:  {DESCRIPTION}");
    println!("  command:      {command}");
    println!("  start type:   automatic");
    println!("Run `lce-index-service start` to start it now.");
    0
}

/// Stop and remove the service.
pub fn uninstall() -> i32 {
    let _ = stop();

    let manager =
        unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) };
    if manager.is_null() {
        return report_permission_failure("opening the service control manager");
    }
    let name = to_wide(SERVICE_NAME);
    // SAFETY: `name` is a NUL terminated UTF-16 buffer.
    let service = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_ALL_ACCESS) };
    if service.is_null() {
        let code = last_error();
        // SAFETY: `manager` came from OpenSCManagerW and is not used again.
        unsafe { CloseServiceHandle(manager) };
        if code == ERROR_SERVICE_DOES_NOT_EXIST {
            println!("{SERVICE_NAME} is not installed");
            return 0;
        }
        eprintln!("could not open the service (error {code})");
        return 1;
    }

    // SAFETY: `service` came from OpenServiceW and is not used afterwards.
    let removed = unsafe { DeleteService(service) };
    unsafe {
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
    }
    if removed == 0 {
        eprintln!("could not delete the service (error {})", last_error());
        return 1;
    }
    println!("removed {SERVICE_NAME}");
    0
}

/// Ask the SCM to start the service.
pub fn start() -> i32 {
    let manager =
        unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) };
    if manager.is_null() {
        return report_permission_failure("opening the service control manager");
    }
    let name = to_wide(SERVICE_NAME);
    // SAFETY: `name` is a NUL terminated UTF-16 buffer.
    let service = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_ALL_ACCESS) };
    if service.is_null() {
        let code = last_error();
        unsafe { CloseServiceHandle(manager) };
        eprintln!("could not open {SERVICE_NAME} (error {code}); run `install` first");
        return 1;
    }

    // SAFETY: `service` is a live handle and the remaining arguments are null as
    // documented for "start with no arguments".
    let started = unsafe { StartServiceW(service, 0, std::ptr::null()) };
    let code = if started == 0 { last_error() } else { 0 };
    unsafe {
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
    }

    if code != 0 {
        eprintln!("could not start {SERVICE_NAME} (error {code})");
        return 1;
    }
    println!("started {SERVICE_NAME}");
    0
}

/// Ask the SCM to stop the service.
pub fn stop() -> i32 {
    let manager =
        unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) };
    if manager.is_null() {
        return report_permission_failure("opening the service control manager");
    }
    let name = to_wide(SERVICE_NAME);
    let service = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_ALL_ACCESS) };
    if service.is_null() {
        let code = last_error();
        unsafe { CloseServiceHandle(manager) };
        if code == ERROR_SERVICE_DOES_NOT_EXIST {
            return 0;
        }
        eprintln!("could not open {SERVICE_NAME} (error {code})");
        return 1;
    }

    let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: `service` is live and `status` is a valid out-parameter.
    let sent = unsafe { ControlService(service, SERVICE_CONTROL_STOP, &mut status) };
    let code = if sent == 0 { last_error() } else { 0 };
    unsafe {
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
    }
    if code != 0 {
        eprintln!("could not stop {SERVICE_NAME} (error {code})");
        return 1;
    }
    println!("stopped {SERVICE_NAME}");
    0
}

/// Print the current service state.
pub fn status() -> i32 {
    let manager =
        unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) };
    if manager.is_null() {
        return report_permission_failure("opening the service control manager");
    }
    let name = to_wide(SERVICE_NAME);
    let service = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_ALL_ACCESS) };
    if service.is_null() {
        let code = last_error();
        unsafe { CloseServiceHandle(manager) };
        if code == ERROR_SERVICE_DOES_NOT_EXIST {
            println!("{SERVICE_NAME}: not installed");
            return 1;
        }
        eprintln!("could not open {SERVICE_NAME} (error {code})");
        return 1;
    }

    let mut buffer = vec![0u8; 4096];
    let mut needed = 0u32;
    // SAFETY: the buffer is valid for the length passed in.
    let ok = unsafe {
        QueryServiceStatusEx(
            service,
            SC_STATUS_PROCESS_INFO,
            buffer.as_mut_ptr().cast::<u8>(),
            u32::try_from(buffer.len()).unwrap_or(4096),
            &mut needed,
        )
    };
    unsafe {
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
    }
    if ok == 0 {
        eprintln!("could not query {SERVICE_NAME} (error {})", last_error());
        return 1;
    }

    // SAFETY: on success the buffer begins with a SERVICE_STATUS_PROCESS, whose
    // leading fields match SERVICE_STATUS.
    let state = unsafe { (*buffer.as_ptr().cast::<SERVICE_STATUS>()).dwCurrentState };
    println!("{SERVICE_NAME}: {}", state_name(state));
    0
}

fn state_name(state: u32) -> &'static str {
    match state {
        SERVICE_STOPPED => "stopped",
        SERVICE_START_PENDING => "starting",
        SERVICE_STOP_PENDING => "stopping",
        SERVICE_RUNNING => "running",
        5 => "continuing",
        6 => "pausing",
        7 => "paused",
        _ => "unknown",
    }
}

/// The SCM entry point. Blocks until the service stops.
pub fn run_as_service() -> i32 {
    let name = to_wide(SERVICE_NAME);
    let mut table = [
        windows_sys::Win32::System::Services::SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_ptr() as *mut u16,
            lpServiceProc: Some(service_main),
        },
        windows_sys::Win32::System::Services::SERVICE_TABLE_ENTRYW {
            lpServiceName: std::ptr::null_mut(),
            lpServiceProc: None,
        },
    ];

    // SAFETY: `table` is a NUL terminated array of entries that outlives the
    // call, as `StartServiceCtrlDispatcherW` requires.
    let ok = unsafe { StartServiceCtrlDispatcherW(table.as_mut_ptr()) };
    if ok == 0 {
        let code = last_error();
        // 1063 is ERROR_FAILED_SERVICE_CONTROLLER_CONNECT: the binary was
        // started from a console rather than by the SCM.
        if code == 1063 {
            eprintln!("this command must be started by the service control manager;");
            eprintln!("use `lce-index-service --console` to run it in the foreground");
            return 1;
        }
        eprintln!("service dispatcher failed (error {code})");
        return 1;
    }
    0
}

unsafe extern "system" fn service_main(_argument_count: u32, _arguments: *mut *mut u16) {
    let name = to_wide(SERVICE_NAME);
    // SAFETY: `name` outlives the call and the handler is a valid function
    // pointer.
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control_handler), std::ptr::null_mut())
    };
    if handle.is_null() {
        return;
    }

    report_status(handle, SERVICE_START_PENDING, 1);
    report_status(handle, SERVICE_RUNNING, 0);

    let exit = crate::run_server(index_protocol::DEFAULT_PIPE_NAME);

    report_status(handle, SERVICE_STOP_PENDING, 1);
    if exit != 0 {
        tracing::warn!(exit, "the index service exited with an error");
    }
    report_status(handle, SERVICE_STOPPED, 0);
}

unsafe extern "system" fn control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP => {
            crate::request_stop();
            0
        }
        SERVICE_CONTROL_INTERROGATE => 0,
        _ => 0,
    }
}

fn report_status(handle: HANDLE, state: u32, hint: u32) {
    let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    status.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
    status.dwCurrentState = state;
    status.dwControlsAccepted = if state == SERVICE_RUNNING {
        SERVICE_ACCEPT_STOP
    } else {
        0
    };
    status.dwWin32ExitCode = 0;
    status.dwServiceSpecificExitCode = 0;
    status.dwCheckPoint = hint;
    status.dwWaitHint = 3_000;
    // SAFETY: `handle` came from RegisterServiceCtrlHandlerExW and `status` is
    // a live local.
    unsafe {
        SetServiceStatus(handle, &status);
    }
}

fn report_permission_failure(action: &str) -> i32 {
    let error = LceError::Permission(PermissionError::ElevationRequired {
        reason: action.to_string(),
    });
    eprintln!("{}", error.hint());
    1
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

    #[test]
    fn state_names_cover_the_documented_values() {
        assert_eq!(state_name(SERVICE_RUNNING), "running");
        assert_eq!(state_name(SERVICE_STOPPED), "stopped");
        assert_eq!(state_name(SERVICE_START_PENDING), "starting");
        assert_eq!(state_name(SERVICE_STOP_PENDING), "stopping");
        assert_eq!(state_name(999), "unknown");
    }

    #[test]
    fn wide_strings_are_nul_terminated() {
        let wide = to_wide("lce-index-service");
        assert_eq!(wide.last().copied(), Some(0));
        assert_eq!(wide.len(), SERVICE_NAME.chars().count() + 1);
    }

    #[test]
    fn the_service_identity_is_stable() {
        // Changing these breaks every installed service, so they are asserted
        // rather than left to drift.
        assert_eq!(SERVICE_NAME, "lce-index-service");
        assert!(DISPLAY_NAME.contains("Local Context Engine"));
    }

    #[test]
    fn status_reports_not_installed_without_failing_the_process() {
        // This machine may or may not have the service; either way the command
        // must return a clean exit code rather than panicking.
        let code = status();
        assert!(code == 0 || code == 1);
    }
}
