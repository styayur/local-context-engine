//! Named Pipe transport with an explicit DACL.
//!
//! The DACL is built from an SDDL string rather than assembled by hand, because
//! a hand-built `ACL` is easy to get subtly wrong and impossible to review:
//!
//! ```text
//! D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;<the installing user's SID>)
//! ```
//!
//! * `SY` — LocalSystem, which owns the service.
//! * `BA` — the built-in Administrators group, which may manage it.
//! * the installation user's SID — read and write, which is what a front end
//!   needs to ask a question.
//!
//! Notably absent: `WD` (Everyone) and `AN` (Anonymous). A default-ACL pipe
//! would grant both, and that is exactly the mistake this module exists to
//! avoid.

use std::ffi::c_void;
use std::fs::File;
use std::os::windows::io::{FromRawHandle, RawHandle};

use search_core::{LceError, PlatformError};
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::FlushFileBuffers;
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// SDDL revision 1. The only revision Windows accepts.
const SDDL_REVISION_1: u32 = 1;
// Pipe mode flags, from winbase.h / namedpipeapi.h. Defined here rather than
// imported because windows-sys scatters them across modules by header.
const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
/// Byte stream, not message stream.
///
/// This is deliberate. In message mode a `ReadFile` that returns fewer bytes
/// than the message *discards the rest of it*, which is fatal for a
/// length-prefixed framing scheme that reads a header and then a body. Byte
/// mode lets both sides treat the pipe as an ordinary `Read + Write` stream,
/// and the protocol supplies its own message boundaries.
const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
const PIPE_WAIT: u32 = 0x0000_0000;
const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x0000_0008;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;
/// `FILE_FLAG_FIRST_PIPE_INSTANCE`: fail rather than let another process squat
/// on our pipe name.
const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
/// `ERROR_PIPE_CONNECTED`: a client was already there when we called
/// `ConnectNamedPipe`.
const ERROR_PIPE_CONNECTED: u32 = 535;

/// A listening Named Pipe server.
#[derive(Debug)]
pub struct PipeServer {
    handle: HANDLE,
    stream: File,
    name: String,
    /// Whether the current pipe instance has a client attached.
    connected: bool,
}

// SAFETY: the handle is only used through `&mut self` methods, and `File` is
// itself `Send`. Nothing here is shared between threads without `&mut`.
unsafe impl Send for PipeServer {}

impl PipeServer {
    /// Create the first instance of a pipe with the restrictive DACL above.
    pub fn create(pipe_name: &str, buffer_size: u32) -> Result<Self, LceError> {
        let sddl = build_sddl()?;
        let (descriptor, attributes) = security_attributes(&sddl)?;

        let path = pipe_path(pipe_name);
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

        // SAFETY: `wide` is NUL terminated, `attributes` points at a live
        // SECURITY_ATTRIBUTES whose descriptor outlives the call.
        let handle = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                buffer_size,
                buffer_size,
                0,
                &attributes,
            )
        };

        // SAFETY: the descriptor was allocated by
        // ConvertStringSecurityDescriptorToSecurityDescriptorW and is no longer
        // needed once the pipe exists.
        unsafe {
            LocalFree(descriptor);
        }

        if handle.is_null() || handle as isize == -1 {
            return Err(LceError::Platform(PlatformError::WindowsApi {
                call: "CreateNamedPipeW".into(),
                code: i64::from(last_error()),
            }));
        }

        // SAFETY: `handle` came from CreateNamedPipeW and ownership is being
        // transferred to `File`, which closes it exactly once.
        let stream = unsafe { File::from_raw_handle(handle as RawHandle) };
        Ok(Self {
            handle,
            stream,
            name: pipe_name.to_string(),
            connected: false,
        })
    }

    /// The pipe name, without the `\\.\pipe\` prefix.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The raw handle, for `ConnectNamedPipe`/`DisconnectNamedPipe`.
    #[must_use]
    pub fn handle(&self) -> HANDLE {
        self.handle
    }

    /// Block until a client connects.
    ///
    /// Returns `Ok(true)` when a client is attached, `Ok(false)` when the
    /// caller should stop instead.
    pub fn accept(&mut self) -> Result<bool, LceError> {
        if self.connected {
            return Ok(true);
        }
        // SAFETY: `handle` is a live pipe handle owned by this struct.
        let ok = unsafe { ConnectNamedPipe(self.handle, std::ptr::null_mut()) };
        if ok != 0 {
            self.connected = true;
            return Ok(true);
        }
        let code = last_error();
        if code == ERROR_PIPE_CONNECTED {
            self.connected = true;
            return Ok(true);
        }
        Err(LceError::Platform(PlatformError::WindowsApi {
            call: "ConnectNamedPipe".into(),
            code: i64::from(code),
        }))
    }

    /// The connected stream.
    pub fn stream(&mut self) -> &mut File {
        &mut self.stream
    }

    /// Detach the current client, ready for the next one.
    pub fn disconnect(&mut self) {
        if !self.connected {
            return;
        }
        // SAFETY: `handle` is a live pipe handle owned by this struct. A
        // failure here only means the client already went away.
        unsafe {
            let _ = FlushFileBuffers(self.handle);
            let _ = DisconnectNamedPipe(self.handle);
        }
        self.connected = false;
    }

    /// Release the handle.
    ///
    /// The handle is owned by the embedded `File`, which closes it on drop;
    /// this only drops our raw copy so no later call can use a stale handle.
    pub fn release(&mut self) {
        self.disconnect();
        self.handle = std::ptr::null_mut();
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        // `File` closes the handle; only disconnect the client.
        self.disconnect();
    }
}

/// Build the SDDL string for this user, SYSTEM and Administrators.
pub fn build_sddl() -> Result<String, LceError> {
    let user = current_user_sid_string()?;
    Ok(format!("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;{user})"))
}

fn security_attributes(sddl: &str) -> Result<(*mut c_void, SECURITY_ATTRIBUTES), LceError> {
    let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor: *mut c_void = std::ptr::null_mut();
    let mut size = 0u32;

    // SAFETY: `wide` is NUL terminated and the two out-parameters are live.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            &mut size,
        )
    };
    if ok == 0 || descriptor.is_null() {
        return Err(LceError::Platform(PlatformError::WindowsApi {
            call: "ConvertStringSecurityDescriptorToSecurityDescriptorW".into(),
            code: i64::from(last_error()),
        }));
    }

    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(24),
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    Ok((descriptor, attributes))
}

/// The current process user's SID as an SDDL string.
pub fn current_user_sid_string() -> Result<String, LceError> {
    // SAFETY: GetCurrentProcess returns a pseudo handle that is always valid
    // and must not be closed.
    let process = unsafe { GetCurrentProcess() };
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `token` is a live out-parameter.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(LceError::Platform(PlatformError::WindowsApi {
            call: "OpenProcessToken".into(),
            code: i64::from(last_error()),
        }));
    }

    let result = token_sid_string(token);
    // SAFETY: `token` came from OpenProcessToken and is not used again.
    unsafe { CloseHandle(token) };
    result
}

fn token_sid_string(token: HANDLE) -> Result<String, LceError> {
    let mut needed = 0u32;
    // SAFETY: a null buffer with length zero is the documented size probe.
    unsafe {
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 64 * 1024 {
        return Err(LceError::Platform(PlatformError::WindowsApi {
            call: "GetTokenInformation(TokenUser) size probe".into(),
            code: i64::from(last_error()),
        }));
    }

    let mut buffer = vec![0u8; needed as usize];
    // SAFETY: the buffer is exactly `needed` bytes.
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
        return Err(LceError::Platform(PlatformError::WindowsApi {
            call: "GetTokenInformation(TokenUser)".into(),
            code: i64::from(last_error()),
        }));
    }

    // SAFETY: on success the buffer holds a TOKEN_USER whose SID pointer is
    // owned by the same buffer.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    if sid.is_null() {
        return Err(LceError::Platform(PlatformError::NotFound {
            resource: "the current user's SID".into(),
        }));
    }

    let mut string_sid: *mut u16 = std::ptr::null_mut();
    // SAFETY: `sid` is valid and `string_sid` is a live out-parameter.
    let ok = unsafe { ConvertSidToStringSidW(sid, &mut string_sid) };
    if ok == 0 || string_sid.is_null() {
        return Err(LceError::Platform(PlatformError::WindowsApi {
            call: "ConvertSidToStringSidW".into(),
            code: i64::from(last_error()),
        }));
    }

    let mut length = 0usize;
    // SAFETY: the API guarantees a NUL terminated string; the cap stops a
    // malformed pointer from running away.
    while length < 256 && unsafe { *string_sid.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` units were verified readable above.
    let slice = unsafe { std::slice::from_raw_parts(string_sid, length) };
    let sid_string = String::from_utf16_lossy(slice);

    // SAFETY: `string_sid` was allocated by ConvertSidToStringSidW with
    // LocalAlloc and is freed exactly once here.
    unsafe {
        LocalFree(string_sid as *mut c_void);
    }
    Ok(sid_string)
}

/// The full pipe path for a name.
#[must_use]
pub fn pipe_path(pipe_name: &str) -> String {
    format!(r"\\.\pipe\{pipe_name}")
}

fn last_error() -> u32 {
    // SAFETY: GetLastError is always safe to call.
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sddl_grants_system_administrators_and_this_user_only() {
        let sddl = build_sddl().expect("this user must have a SID");
        assert!(sddl.starts_with("D:("));
        assert!(sddl.contains("(A;;GA;;;SY)"), "{sddl}");
        assert!(sddl.contains("(A;;GA;;;BA)"), "{sddl}");
        assert!(
            sddl.contains("S-1-"),
            "the user SID must be present: {sddl}"
        );
        // The two entries that turn a defined pipe into a hole.
        assert!(
            !sddl.contains(";;;WD)"),
            "Everyone must not be granted: {sddl}"
        );
        assert!(
            !sddl.contains(";;;AN)"),
            "Anonymous must not be granted: {sddl}"
        );
    }

    #[test]
    fn the_current_user_sid_is_resolvable() {
        let sid = current_user_sid_string().expect("SID");
        assert!(sid.starts_with("S-1-"), "{sid}");
    }

    #[test]
    fn a_pipe_can_be_created_with_that_descriptor() {
        // The name is unique per process so parallel test runs cannot collide.
        let name = format!("lce-pipe-create-{}", std::process::id());
        let server = PipeServer::create(&name, 4096).expect("pipe creation");
        assert_eq!(server.name(), &name);
    }
}
