//! # `index-client`
//!
//! The unprivileged side of the IPC boundary.
//!
//! Every front end uses this type and nothing else to talk to
//! `lce-index-service.exe`. There is no second implementation of the pipe
//! handshake hiding in the CLI, the MCP server or the desktop shell.
//!
//! Opening `\\.\pipe\<name>` is ordinary file I/O on Windows, so the client
//! needs no FFI and no unsafe code at all.

#![forbid(unsafe_code)]

use std::fs::File;
use std::io::Write;
use std::time::Duration;

use index_protocol::{
    read_frame, ProtocolError, Request, Response, DEFAULT_PIPE_NAME, PROTOCOL_VERSION,
};
use search_core::LceError;

/// Client name reported in the handshake.
pub const CLIENT_NAME: &str = concat!("localsearch-", env!("CARGO_PKG_VERSION"));

/// A connected index service.
#[derive(Debug)]
pub struct IndexClient {
    pipe: File,
    pipe_name: String,
}

impl IndexClient {
    /// Connect to the default pipe.
    pub fn connect() -> Result<Self, LceError> {
        Self::connect_to(DEFAULT_PIPE_NAME)
    }

    /// Connect to a specific pipe name (without the `\\.\pipe\` prefix).
    pub fn connect_to(pipe_name: &str) -> Result<Self, LceError> {
        let path = pipe_path(pipe_name);
        let pipe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| LceError::Io {
                action: format!("connecting to the index service at {path}"),
                detail: error.to_string(),
            })?;
        Ok(Self {
            pipe,
            pipe_name: pipe_name.to_string(),
        })
    }

    /// Whether the service is running and accepting connections.
    #[must_use]
    pub fn is_available() -> bool {
        Self::is_available_on(DEFAULT_PIPE_NAME)
    }

    /// Whether a specific pipe is accepting connections.
    #[must_use]
    pub fn is_available_on(pipe_name: &str) -> bool {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(pipe_path(pipe_name))
            .is_ok()
    }

    /// The pipe this client is connected to.
    #[must_use]
    pub fn pipe_name(&self) -> &str {
        &self.pipe_name
    }

    /// Send one request and read one response.
    pub fn request(&mut self, request: &Request) -> Result<Response, LceError> {
        let frame = index_protocol::encode(request).map_err(protocol_error)?;
        self.pipe
            .write_all(&frame)
            .and_then(|()| self.pipe.flush())
            .map_err(|error| LceError::Io {
                action: "writing to the index service".into(),
                detail: error.to_string(),
            })?;

        let frame = read_frame(&mut self.pipe)
            .map_err(protocol_error)?
            .ok_or_else(|| LceError::Io {
                action: "reading from the index service".into(),
                detail: "the service closed the connection".into(),
            })?;
        index_protocol::decode(&frame).map_err(protocol_error)
    }

    /// Perform the handshake, returning the service's self-description.
    pub fn hello(&mut self) -> Result<Response, LceError> {
        self.request(&Request::Hello {
            protocol_version: PROTOCOL_VERSION,
            client: CLIENT_NAME.to_string(),
        })
    }

    /// Handshake and fail unless the service agrees on the protocol revision.
    pub fn handshake(&mut self) -> Result<Response, LceError> {
        let response = self.hello()?;
        match &response {
            Response::Hello {
                protocol_version, ..
            } if *protocol_version == PROTOCOL_VERSION => Ok(response),
            Response::Hello {
                protocol_version, ..
            } => Err(LceError::Platform(search_core::PlatformError::Unsupported {
                feature: format!(
                    "index service protocol {protocol_version} (this build speaks {PROTOCOL_VERSION})"
                ),
            })),
            Response::Error { code, message } => Err(LceError::Io {
                action: "the index service rejected the handshake".into(),
                detail: format!("{code}: {message}"),
            }),
            other => Err(LceError::Io {
                action: "the index service handshake".into(),
                detail: format!("unexpected response `{}`", other.kind()),
            }),
        }
    }
}

/// Try to connect, retrying briefly.
///
/// A service that was just started may not have created its pipe yet, and
/// sleeping one short interval beats surfacing a confusing I/O error.
#[must_use]
pub fn connect_with_retry(attempts: u32, delay: Duration) -> Option<IndexClient> {
    for attempt in 0..attempts.max(1) {
        if let Ok(client) = IndexClient::connect() {
            return Some(client);
        }
        if attempt + 1 < attempts {
            std::thread::sleep(delay);
        }
    }
    None
}

/// The full pipe path for a name.
#[must_use]
pub fn pipe_path(pipe_name: &str) -> String {
    format!(r"\\.\pipe\{pipe_name}")
}

fn protocol_error(error: ProtocolError) -> LceError {
    LceError::Io {
        action: "talking to the index service".into(),
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_paths_use_the_windows_prefix() {
        assert_eq!(pipe_path("demo"), r"\\.\pipe\demo");
        assert!(pipe_path(DEFAULT_PIPE_NAME).starts_with(r"\\.\pipe\"));
    }

    #[test]
    fn a_missing_service_is_reported_as_unavailable_not_as_a_panic() {
        assert!(
            !IndexClient::is_available_on("localsearch-no-such-pipe-4f8a2c"),
            "a pipe that does not exist must simply be unavailable"
        );
    }

    #[test]
    fn connecting_to_a_missing_service_is_an_io_error() {
        let error = IndexClient::connect_to("localsearch-no-such-pipe-4f8a2c").unwrap_err();
        assert_eq!(error.code(), "io-error");
    }

    #[test]
    fn retrying_gives_up_cleanly() {
        let client = connect_with_retry(2, Duration::from_millis(10));
        assert!(client.is_none());
    }

    #[test]
    fn the_client_name_carries_the_version() {
        assert!(CLIENT_NAME.starts_with("localsearch-"));
    }
}
