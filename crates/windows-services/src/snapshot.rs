//! Raw service enumeration via the service control manager.

use search_core::{
    LceError, PermissionError, PlatformError, ServiceEntry, ServiceStartType, ServiceState,
};
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW, QueryServiceConfigW,
    ENUM_SERVICE_STATUS_PROCESSW, QUERY_SERVICE_CONFIGW, SC_ENUM_PROCESS_INFO, SC_HANDLE,
    SC_MANAGER_ENUMERATE_SERVICE, SERVICE_QUERY_CONFIG, SERVICE_STATE_ALL,
};

/// `SERVICE_WIN32_OWN_PROCESS | SERVICE_WIN32_SHARE_PROCESS`.
const SERVICE_WIN32_TYPE: u32 = 0x30;
/// `ERROR_MORE_DATA` from winerror.h: the buffer was too small.
const ERROR_MORE_DATA: u32 = 234;
const INITIAL_BUFFER: usize = 64 * 1024;
const MAX_BUFFER: usize = 16 * 1024 * 1024;

// Start types, from winsvc.h.
const SERVICE_BOOT_START: u32 = 0;
const SERVICE_SYSTEM_START: u32 = 1;
const SERVICE_AUTO_START: u32 = 2;
const SERVICE_DEMAND_START: u32 = 3;
const SERVICE_DISABLED: u32 = 4;

// Current states, from winsvc.h.
const SERVICE_STOPPED: u32 = 1;
const SERVICE_START_PENDING: u32 = 2;
const SERVICE_STOP_PENDING: u32 = 3;
const SERVICE_RUNNING: u32 = 4;
const SERVICE_CONTINUE_PENDING: u32 = 5;
const SERVICE_PAUSE_PENDING: u32 = 6;
const SERVICE_PAUSED: u32 = 7;

/// A hint about what the caller is going to do with the service list.
///
/// Resolving `QueryServiceConfig` costs two calls per service. When the caller
/// is searching by name and does not filter on binary path or start type, the
/// config for non-matching services is never observed, so it is not fetched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceQueryHint {
    /// Lower-cased text tokens the caller will match against.
    pub text_tokens: Vec<String>,
    /// Whether every service needs its config regardless of the text match.
    pub needs_full_config: bool,
}

impl ServiceQueryHint {
    /// A hint that resolves every service's configuration.
    #[must_use]
    pub fn full() -> Self {
        Self {
            text_tokens: Vec::new(),
            needs_full_config: true,
        }
    }

    /// Whether this hint calls for resolving the given service's config.
    #[must_use]
    pub fn wants_config(&self, name: &str, display_name: &str) -> bool {
        if self.needs_full_config || self.text_tokens.is_empty() {
            return true;
        }
        let lowered_name = name.to_lowercase();
        let lowered_display = display_name.to_lowercase();
        self.text_tokens.iter().any(|token| {
            lowered_name.contains(token.as_str()) || lowered_display.contains(token.as_str())
        })
    }
}

/// Enumerate every Win32 service visible to the caller, resolving all details.
pub fn list_services() -> Result<Vec<ServiceEntry>, LceError> {
    list_services_with(&ServiceQueryHint::full())
}

/// Enumerate services, using `hint` to decide how much per-service work to do.
pub fn list_services_with(hint: &ServiceQueryHint) -> Result<Vec<ServiceEntry>, LceError> {
    // SAFETY: null machine/database means "the local machine"; the returned
    // handle is checked and closed on every path below.
    let scm = unsafe {
        OpenSCManagerW(
            std::ptr::null(),
            std::ptr::null(),
            SC_MANAGER_ENUMERATE_SERVICE,
        )
    };
    if scm.is_null() {
        return Err(permission_error());
    }

    let entries = match enumerate(scm, hint) {
        Ok(entries) => entries,
        Err(error) => {
            // SAFETY: `scm` came from OpenSCManagerW and is not used again.
            unsafe { CloseServiceHandle(scm) };
            return Err(error);
        }
    };
    // SAFETY: `scm` came from OpenSCManagerW and is not used again.
    unsafe { CloseServiceHandle(scm) };
    Ok(entries)
}

fn enumerate(scm: SC_HANDLE, hint: &ServiceQueryHint) -> Result<Vec<ServiceEntry>, LceError> {
    let mut buffer = vec![0u8; INITIAL_BUFFER];
    let mut bytes_needed = 0u32;
    let mut returned = 0u32;

    loop {
        let mut resume = 0u32;
        // SAFETY: `buffer` is a valid writable allocation of `buffer.len()`
        // bytes and every out-parameter points at a live local.
        let ok = unsafe {
            EnumServicesStatusExW(
                scm,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32_TYPE,
                SERVICE_STATE_ALL,
                buffer.as_mut_ptr(),
                u32::try_from(buffer.len()).unwrap_or(u32::MAX),
                &mut bytes_needed,
                &mut returned,
                &mut resume,
                std::ptr::null(),
            )
        };

        if ok != 0 {
            break;
        }
        let code = last_error();
        let wanted = bytes_needed as usize;
        if code == ERROR_MORE_DATA && wanted > buffer.len() && wanted <= MAX_BUFFER {
            buffer.resize(wanted, 0);
            continue;
        }
        return Err(LceError::Platform(PlatformError::WindowsApi {
            call: "EnumServicesStatusExW".into(),
            code: i64::from(code),
        }));
    }

    // SAFETY: on success the API wrote `returned` ENUM_SERVICE_STATUS_PROCESSW
    // records into the front of `buffer`, which is aligned for them.
    let records = unsafe {
        std::slice::from_raw_parts(
            buffer.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>(),
            returned as usize,
        )
    };

    let mut services = Vec::with_capacity(records.len());
    for record in records {
        let name = wide_pointer_to_string(record.lpServiceName);
        if name.is_empty() {
            continue;
        }
        let display_name = wide_pointer_to_string(record.lpDisplayName);
        let config = if hint.wants_config(&name, &display_name) {
            query_config(scm, &name)
        } else {
            None
        };

        services.push(ServiceEntry {
            display_name: if display_name.is_empty() {
                name.clone()
            } else {
                display_name
            },
            state: map_state(record.ServiceStatusProcess.dwCurrentState),
            pid: (record.ServiceStatusProcess.dwProcessId != 0)
                .then_some(record.ServiceStatusProcess.dwProcessId),
            binary_path: config
                .as_ref()
                .map(|config| config.binary_path.clone())
                .filter(|path| !path.is_empty()),
            start_type: config
                .as_ref()
                .map_or(ServiceStartType::Unknown, |config| config.start_type),
            account: config
                .as_ref()
                .and_then(|config| config.account.clone())
                .filter(|account| !account.is_empty()),
            name,
        });
    }

    Ok(services)
}

#[derive(Debug, Default)]
struct ServiceConfig {
    binary_path: String,
    start_type: ServiceStartType,
    account: Option<String>,
}

fn query_config(scm: SC_HANDLE, name: &str) -> Option<ServiceConfig> {
    let wide_name = to_wide(name);
    // SAFETY: `wide_name` is a NUL terminated UTF-16 buffer that outlives the call.
    let service = unsafe { OpenServiceW(scm, wide_name.as_ptr(), SERVICE_QUERY_CONFIG) };
    if service.is_null() {
        return None;
    }

    let config = read_config(service);
    // SAFETY: `service` came from OpenServiceW and is not used again.
    unsafe { CloseServiceHandle(service) };
    config
}

fn read_config(service: SC_HANDLE) -> Option<ServiceConfig> {
    let mut needed = 0u32;
    // SAFETY: a null buffer with size 0 is the documented size probe.
    unsafe {
        QueryServiceConfigW(service, std::ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed as usize > 1024 * 1024 {
        return None;
    }

    let mut buffer = vec![0u8; needed as usize];
    // SAFETY: the buffer is exactly `needed` bytes, and the returned structure
    // points into that same buffer, so it must be read before it is dropped.
    let ok = unsafe {
        QueryServiceConfigW(
            service,
            buffer.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>(),
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return None;
    }

    // SAFETY: on success the buffer holds a QUERY_SERVICE_CONFIGW whose string
    // pointers are owned by the same buffer.
    let raw = unsafe { &*buffer.as_ptr().cast::<QUERY_SERVICE_CONFIGW>() };

    Some(ServiceConfig {
        binary_path: wide_pointer_to_string(raw.lpBinaryPathName),
        start_type: map_start_type(raw.dwStartType),
        account: {
            let account = wide_pointer_to_string(raw.lpServiceStartName);
            (!account.is_empty()).then_some(account)
        },
    })
}

const fn map_state(raw: u32) -> ServiceState {
    match raw {
        SERVICE_STOPPED => ServiceState::Stopped,
        SERVICE_START_PENDING => ServiceState::StartPending,
        SERVICE_STOP_PENDING => ServiceState::StopPending,
        SERVICE_RUNNING => ServiceState::Running,
        SERVICE_CONTINUE_PENDING => ServiceState::ContinuePending,
        SERVICE_PAUSE_PENDING => ServiceState::PausePending,
        SERVICE_PAUSED => ServiceState::Paused,
        _ => ServiceState::Unknown,
    }
}

const fn map_start_type(raw: u32) -> ServiceStartType {
    match raw {
        SERVICE_BOOT_START => ServiceStartType::Boot,
        SERVICE_SYSTEM_START => ServiceStartType::System,
        SERVICE_AUTO_START => ServiceStartType::Automatic,
        SERVICE_DEMAND_START => ServiceStartType::Manual,
        SERVICE_DISABLED => ServiceStartType::Disabled,
        _ => ServiceStartType::Unknown,
    }
}

/// Copy a NUL terminated UTF-16 string out of a Win32 buffer.
fn wide_pointer_to_string(pointer: *const u16) -> String {
    if pointer.is_null() {
        return String::new();
    }
    let mut length = 0usize;
    // SAFETY: the API guarantees a NUL terminated string; the length cap keeps
    // a malformed pointer from running away.
    while length < 32_768 {
        let unit = unsafe { *pointer.add(length) };
        if unit == 0 {
            break;
        }
        length += 1;
    }
    // SAFETY: `length` units were verified readable above.
    let slice = unsafe { std::slice::from_raw_parts(pointer, length) };
    String::from_utf16_lossy(slice)
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn permission_error() -> LceError {
    LceError::Permission(PermissionError::AccessDenied {
        action: "opening the service control manager".into(),
    })
}

fn last_error() -> u32 {
    // SAFETY: GetLastError is always safe to call.
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_states_map_from_winsvc_constants() {
        assert_eq!(map_state(SERVICE_RUNNING), ServiceState::Running);
        assert_eq!(map_state(SERVICE_STOPPED), ServiceState::Stopped);
        assert_eq!(map_state(SERVICE_PAUSED), ServiceState::Paused);
        assert_eq!(map_state(999), ServiceState::Unknown);
    }

    #[test]
    fn start_types_map_from_winsvc_constants() {
        assert_eq!(
            map_start_type(SERVICE_AUTO_START),
            ServiceStartType::Automatic
        );
        assert_eq!(
            map_start_type(SERVICE_DEMAND_START),
            ServiceStartType::Manual
        );
        assert_eq!(map_start_type(SERVICE_DISABLED), ServiceStartType::Disabled);
        assert_eq!(map_start_type(42), ServiceStartType::Unknown);
    }

    #[test]
    fn wide_pointer_decoding_handles_nulls_and_content() {
        assert_eq!(wide_pointer_to_string(std::ptr::null()), "");
        let buffer = to_wide("Windows Update");
        assert_eq!(wide_pointer_to_string(buffer.as_ptr()), "Windows Update");
    }

    #[test]
    fn wide_pointer_decoding_handles_unicode() {
        let buffer = to_wide("服务 Service");
        assert_eq!(wide_pointer_to_string(buffer.as_ptr()), "服务 Service");
    }

    #[test]
    fn the_service_list_can_be_read_on_this_machine() {
        let services = list_services().expect("enumerating services must not fail");
        assert!(
            !services.is_empty(),
            "every Windows installation has services"
        );
        assert!(services.iter().any(|service| !service.name.is_empty()));
    }
}
