//! Raw process enumeration.

use std::ffi::c_void;

use search_core::{
    clock, file_name_of, LceError, LocalEntity, PermissionError, PlatformError, ProcessEntry,
};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::{
    GetTokenInformation, LookupAccountSidW, TokenUser, PSID, SID_NAME_USE, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

const MAX_IMAGE_PATH: usize = 32_768;

/// Which optional per-process details to resolve.
///
/// Resolving an account name means `OpenProcessToken` plus a SID lookup, which
/// can hit a domain controller and costs tens of milliseconds across a few
/// hundred processes. It is therefore opt-in rather than default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessOptions {
    /// Resolve the owning account name.
    pub username: bool,
    /// Resolve the full image path.
    pub exe_path: bool,
    /// Read the working set size.
    pub memory: bool,
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self {
            username: false,
            exe_path: true,
            memory: true,
        }
    }
}

impl ProcessOptions {
    /// Resolve everything, including the owning account.
    #[must_use]
    pub const fn full() -> Self {
        Self {
            username: true,
            exe_path: true,
            memory: true,
        }
    }
}

/// Enumerate every process the calling user is allowed to see.
///
/// Processes that cannot be opened are still reported, just with less detail.
/// That is normal on Windows and must never fail the whole snapshot.
pub fn list_processes() -> Result<Vec<ProcessEntry>, LceError> {
    list_processes_with(ProcessOptions::default())
}

/// Enumerate every process, resolving the requested optional details.
pub fn list_processes_with(options: ProcessOptions) -> Result<Vec<ProcessEntry>, LceError> {
    // SAFETY: the returned handle is validated before use and closed on every
    // path out of this function.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(LceError::Platform(PlatformError::WindowsApi {
            call: "CreateToolhelp32Snapshot".into(),
            code: last_error(),
        }));
    }

    let mut raw: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    raw.dwSize = size_of_u32::<PROCESSENTRY32W>();

    let mut listed: Vec<Listed> = Vec::with_capacity(256);
    // SAFETY: `raw` is a zeroed PROCESSENTRY32W whose dwSize field is set, which
    // is exactly the contract of Process32FirstW.
    let mut has_entry = unsafe { Process32FirstW(snapshot, &mut raw) } != 0;
    while has_entry {
        let name = wide_to_string(&raw.szExeFile);
        let pid = raw.th32ProcessID;
        listed.push(Listed {
            pid,
            name: if name.is_empty() {
                format!("pid-{pid}")
            } else {
                name
            },
            parent_pid: raw.th32ParentProcessID,
            thread_count: raw.cntThreads,
        });

        raw.dwSize = size_of_u32::<PROCESSENTRY32W>();
        // SAFETY: the API rewrites the buffer, and dwSize is set again above.
        has_entry = unsafe { Process32NextW(snapshot, &mut raw) } != 0;
    }

    // SAFETY: `snapshot` came from CreateToolhelp32Snapshot and is not used again.
    unsafe { CloseHandle(snapshot) };

    let details = resolve_details(&listed, options);
    Ok(listed
        .into_iter()
        .zip(details)
        .map(|(listed, details)| ProcessEntry {
            pid: listed.pid,
            name: listed.name,
            exe_path: details.exe_path,
            parent_pid: (listed.parent_pid != 0).then_some(listed.parent_pid),
            memory_bytes: details.memory_bytes,
            start_time: details.start_time,
            username: details.username,
            thread_count: listed.thread_count,
            session_id: details.session_id,
        })
        .collect())
}

/// A process as reported by Toolhelp, before optional details are resolved.
#[derive(Debug, Clone)]
struct Listed {
    pid: u32,
    name: String,
    parent_pid: u32,
    thread_count: u32,
}

/// Resolve details for every process, in parallel.
///
/// Each `inspect_process` call is four or five syscalls, so at a few hundred
/// processes the serial version spends most of its time waiting on the kernel.
/// Chunking across threads turns ~30 ms into single digits on a normal machine
/// without pulling in an async runtime.
fn resolve_details(listed: &[Listed], options: ProcessOptions) -> Vec<Details> {
    if listed.is_empty() {
        return Vec::new();
    }
    let workers = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .clamp(1, 8)
        .min(listed.len());
    if workers <= 1 {
        return listed
            .iter()
            .map(|process| inspect_process(process.pid, options))
            .collect();
    }

    let chunk_size = listed.len().div_ceil(workers);
    let mut results: Vec<Details> = vec![Details::default(); listed.len()];
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        // `chunks_mut` gives each worker a disjoint slice, which is what makes
        // the borrow checker happy without any unsafe code.
        for (slots, chunk) in results
            .chunks_mut(chunk_size)
            .zip(listed.chunks(chunk_size))
        {
            handles.push(scope.spawn(move || {
                for (slot, process) in slots.iter_mut().zip(chunk.iter()) {
                    *slot = inspect_process(process.pid, options);
                }
            }));
        }
        for handle in handles {
            if handle.join().is_err() {
                tracing::warn!("a process detail worker panicked; some details are missing");
            }
        }
    });
    results
}

/// The account the calling process runs as.
#[must_use]
pub fn current_username() -> Option<String> {
    // SAFETY: GetCurrentProcess returns a pseudo handle that is always valid
    // for this process and must not be closed.
    let process = unsafe { GetCurrentProcess() };
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `token` is a valid out-parameter; on failure it stays null.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let name = token_username(token);
    // SAFETY: `token` came from OpenProcessToken and is not used again.
    unsafe { CloseHandle(token) };
    name
}

#[derive(Debug, Clone, Default)]
struct Details {
    exe_path: Option<String>,
    memory_bytes: u64,
    start_time: Option<i64>,
    username: Option<String>,
    session_id: Option<u32>,
}

fn inspect_process(pid: u32, options: ProcessOptions) -> Details {
    let mut details = Details::default();

    let mut session = 0u32;
    // SAFETY: `session` is a valid out-parameter.
    if unsafe { ProcessIdToSessionId(pid, &mut session) } != 0 {
        details.session_id = Some(session);
    }

    // SAFETY: OpenProcess with a documented access mask. A null handle is
    // handled immediately and never used below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return details;
    }

    if options.exe_path {
        details.exe_path = process_image_path(handle);
    }
    details.start_time = process_start_time(handle);
    if options.memory {
        details.memory_bytes = process_memory(handle);
    }
    if options.username {
        details.username = process_username(handle);
    }

    // SAFETY: `handle` came from OpenProcess and is not used after this point.
    unsafe { CloseHandle(handle) };
    details
}

fn process_image_path(handle: HANDLE) -> Option<String> {
    let mut buffer = vec![0u16; MAX_IMAGE_PATH];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: the buffer is valid for `length` UTF-16 code units.
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) };
    if ok == 0 || length == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..length as usize]))
}

fn process_start_time(handle: HANDLE) -> Option<i64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: all four pointers are valid out-parameters.
    let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    let ticks = filetime_ticks(&creation);
    (ticks > 0).then(|| filetime_to_unix_ms(ticks))
}

fn process_memory(handle: HANDLE) -> u64 {
    let mut counters: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    counters.cb = size_of_u32::<PROCESS_MEMORY_COUNTERS_EX>();
    // SAFETY: GetProcessMemoryInfo accepts a PROCESS_MEMORY_COUNTERS pointer,
    // and PROCESS_MEMORY_COUNTERS_EX begins with that exact structure.
    let ok = unsafe {
        GetProcessMemoryInfo(
            handle,
            std::ptr::addr_of_mut!(counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
    };
    if ok == 0 {
        return 0;
    }
    counters.WorkingSetSize as u64
}

fn process_username(handle: HANDLE) -> Option<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: probing the token of a process handle we already own; failure is
    // expected for protected processes and yields `None`.
    if unsafe { OpenProcessToken(handle, TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let name = token_username(token);
    // SAFETY: `token` came from OpenProcessToken and is not used again.
    unsafe { CloseHandle(token) };
    name
}

fn token_username(token: HANDLE) -> Option<String> {
    let mut needed = 0u32;
    // SAFETY: a null buffer with length zero is the documented size probe.
    unsafe {
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 16 * 1024 {
        return None;
    }

    let mut buffer = vec![0u8; needed as usize];
    // SAFETY: the buffer is exactly `needed` bytes as reported by the probe.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast::<c_void>(),
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return None;
    }

    // SAFETY: on success GetTokenInformation filled the buffer with a
    // TOKEN_USER whose SID pointer is owned by that same buffer.
    let sid: PSID = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    if sid.is_null() {
        return None;
    }
    lookup_account(sid)
}

fn lookup_account(sid: PSID) -> Option<String> {
    let mut name = vec![0u16; 256];
    let mut domain = vec![0u16; 256];
    let mut name_len = u32::try_from(name.len()).ok()?;
    let mut domain_len = u32::try_from(domain.len()).ok()?;
    let mut usage: SID_NAME_USE = 0;

    // SAFETY: all buffers are valid for the lengths passed in.
    let ok = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut usage,
        )
    };
    if ok == 0 {
        return None;
    }

    let account = String::from_utf16_lossy(&name[..name_len as usize]);
    let domain = String::from_utf16_lossy(&domain[..domain_len as usize]);
    if domain.is_empty() {
        Some(account)
    } else {
        Some(format!("{domain}\\{account}"))
    }
}

/// Pack a `FILETIME` into its 64-bit tick count.
#[must_use]
pub fn filetime_ticks(filetime: &FILETIME) -> i64 {
    let combined = (u64::from(filetime.dwHighDateTime) << 32) | u64::from(filetime.dwLowDateTime);
    i64::try_from(combined).unwrap_or(i64::MAX)
}

/// Convert Windows `FILETIME` ticks (100 ns since 1601-01-01) to Unix ms.
#[must_use]
pub const fn filetime_to_unix_ms(filetime: i64) -> i64 {
    const EPOCH_DELTA_TICKS: i64 = 116_444_736_000_000_000;
    (filetime - EPOCH_DELTA_TICKS) / 10_000
}

/// Convert a `SystemTime`-style Unix millisecond value back into FILETIME ticks.
///
/// Exposed so tests can build fixtures without duplicating the constant.
#[must_use]
pub const fn unix_ms_to_filetime(unix_ms: i64) -> i64 {
    const EPOCH_DELTA_TICKS: i64 = 116_444_736_000_000_000;
    unix_ms * 10_000 + EPOCH_DELTA_TICKS
}

fn size_of_u32<T>() -> u32 {
    u32::try_from(std::mem::size_of::<T>()).unwrap_or(u32::MAX)
}

fn wide_to_string(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

fn last_error() -> i64 {
    // SAFETY: GetLastError is always safe to call.
    i64::from(unsafe { windows_sys::Win32::Foundation::GetLastError() })
}

/// Normalise a process entry into the shared entity model.
#[must_use]
pub fn process_entity(entry: ProcessEntry) -> LocalEntity {
    LocalEntity::Process(entry)
}

/// The image name of a process path.
#[must_use]
pub fn image_name_of(path: &str) -> String {
    file_name_of(path)
}

/// Current wall-clock time as Unix milliseconds.
#[must_use]
pub fn now_unix_ms() -> i64 {
    clock::now_ms()
}

/// The error a caller sees when a process cannot be inspected.
#[must_use]
pub fn access_denied(pid: u32) -> LceError {
    LceError::Permission(PermissionError::AccessDenied {
        action: format!("inspecting process {pid}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_epoch_is_converted_correctly() {
        assert_eq!(filetime_to_unix_ms(unix_ms_to_filetime(0)), 0);
        assert_eq!(filetime_to_unix_ms(unix_ms_to_filetime(1_000)), 1_000);
    }

    #[test]
    fn filetime_round_trips_through_ticks() {
        let original = 1_700_000_000_000i64;
        assert_eq!(filetime_to_unix_ms(unix_ms_to_filetime(original)), original);
    }

    #[test]
    fn wide_strings_stop_at_the_nul_terminator() {
        let buffer = [u16::from(b'a'), u16::from(b'b'), 0, u16::from(b'c')];
        assert_eq!(wide_to_string(&buffer), "ab");
    }

    #[test]
    fn wide_strings_without_a_terminator_use_the_whole_buffer() {
        let buffer = [u16::from(b'x'), u16::from(b'y')];
        assert_eq!(wide_to_string(&buffer), "xy");
    }

    #[test]
    fn image_name_helper_handles_paths() {
        assert_eq!(
            image_name_of(r"C:\Windows\System32\notepad.exe"),
            "notepad.exe"
        );
    }
}
