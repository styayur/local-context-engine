//! # `index-protocol`
//!
//! The wire format between the unprivileged front ends (CLI, MCP server,
//! desktop shell) and the privileged index service.
//!
//! ## Shape
//!
//! ```text
//! frame  := u32 length | u32 magic | u32 version | body
//! length := total frame size, including this header
//! body   := MessagePack(Request | Response)
//! ```
//!
//! * `length` counts the whole frame, so a reader can reject an oversized
//!   message before allocating anything for the body.
//! * `magic` distinguishes our stream from anything else that ends up on the
//!   pipe.
//! * `version` is checked on every frame; a mismatch is a clean error, never a
//!   best-effort decode.
//!
//! ## What is deliberately *not* here
//!
//! There is no `Execute`, no `Shell` and no `Command` variant, and there never
//! will be. The privileged side can enumerate the MFT, read change journals and
//! answer index queries. It cannot run a program, evaluate text, or write a
//! file. A protocol that cannot express those operations cannot be tricked into
//! performing them.

#![forbid(unsafe_code)]

use std::io::{Read, Write};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// Frame magic: `LCE1`.
pub const MAGIC: u32 = 0x4C43_4531;
/// Protocol revision. Bumped for any incompatible message change.
pub const PROTOCOL_VERSION: u32 = 1;
/// Largest frame this implementation will read or write, in bytes.
///
/// A search response with several thousand hits is comfortably under this; the
/// limit exists so a hostile or buggy peer cannot make the service allocate
/// gigabytes.
pub const MAX_MESSAGE_SIZE: usize = 8 * 1024 * 1024;
/// Bytes before the body: length, magic and version.
pub const HEADER_SIZE: usize = 12;

/// Default pipe name, without the `\\.\pipe\` prefix.
pub const DEFAULT_PIPE_NAME: &str = "localsearch-index-v1";

/// Everything that can go wrong on the wire.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// The frame announced more bytes than [`MAX_MESSAGE_SIZE`].
    #[error("frame of {size} bytes exceeds the {max} byte limit")]
    TooLarge {
        /// Size the peer announced.
        size: usize,
        /// The configured maximum.
        max: usize,
    },
    /// The magic did not match.
    #[error("bad frame magic {found:#010x}")]
    BadMagic {
        /// What was actually read.
        found: u32,
    },
    /// The peer speaks a different protocol revision.
    #[error("protocol version {found} is not supported (this build speaks {supported})")]
    VersionMismatch {
        /// The peer's version.
        found: u32,
        /// Ours.
        supported: u32,
    },
    /// The stream ended in the middle of a frame.
    #[error("truncated frame")]
    Truncated,
    /// The body was not valid MessagePack for the expected type.
    #[error("malformed body: {detail}")]
    Malformed {
        /// Codec detail, for the developer log.
        detail: String,
    },
    /// I/O failed.
    #[error("io error: {0}")]
    Io(String),
}

impl From<std::io::Error> for ProtocolError {
    fn from(error: std::io::Error) -> Self {
        ProtocolError::Io(error.to_string())
    }
}

impl From<rmp_serde::encode::Error> for ProtocolError {
    fn from(error: rmp_serde::encode::Error) -> Self {
        ProtocolError::Malformed {
            detail: error.to_string(),
        }
    }
}

impl From<rmp_serde::decode::Error> for ProtocolError {
    fn from(error: rmp_serde::decode::Error) -> Self {
        ProtocolError::Malformed {
            detail: error.to_string(),
        }
    }
}

/// Encode a value into one frame.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    let body = rmp_serde::to_vec_named(value)?;
    let length = body
        .len()
        .checked_add(HEADER_SIZE)
        .ok_or(ProtocolError::TooLarge {
            size: usize::MAX,
            max: MAX_MESSAGE_SIZE,
        })?;
    if length > MAX_MESSAGE_SIZE {
        return Err(ProtocolError::TooLarge {
            size: length,
            max: MAX_MESSAGE_SIZE,
        });
    }

    let mut frame = Vec::with_capacity(HEADER_SIZE + body.len());
    frame.extend_from_slice(&(length as u32).to_le_bytes());
    frame.extend_from_slice(&MAGIC.to_le_bytes());
    frame.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// Decode one frame that has already been read.
pub fn decode<T: DeserializeOwned>(frame: &[u8]) -> Result<T, ProtocolError> {
    let (body, _) = split_frame(frame)?;
    Ok(rmp_serde::from_slice(body)?)
}

/// Validate a frame's header and return its body plus the announced length.
pub fn split_frame(frame: &[u8]) -> Result<(&[u8], usize), ProtocolError> {
    if frame.len() < HEADER_SIZE {
        return Err(ProtocolError::Truncated);
    }
    let length = u32::from_le_bytes(frame[0..4].try_into().unwrap_or([0; 4])) as usize;
    let magic = u32::from_le_bytes(frame[4..8].try_into().unwrap_or([0; 4]));
    let version = u32::from_le_bytes(frame[8..12].try_into().unwrap_or([0; 4]));

    if magic != MAGIC {
        return Err(ProtocolError::BadMagic { found: magic });
    }
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::VersionMismatch {
            found: version,
            supported: PROTOCOL_VERSION,
        });
    }
    if length > MAX_MESSAGE_SIZE {
        return Err(ProtocolError::TooLarge {
            size: length,
            max: MAX_MESSAGE_SIZE,
        });
    }
    if frame.len() < length {
        return Err(ProtocolError::Truncated);
    }
    Ok((&frame[HEADER_SIZE..length], length))
}

/// Read one frame from a stream.
///
/// Returns `Ok(None)` at a clean end of stream. A truncated frame is an error,
/// never a silent `None`: a half-written message must not be mistaken for
/// "nothing to say".
pub fn read_frame<R: Read>(reader: &mut R) -> Result<Option<Vec<u8>>, ProtocolError> {
    let mut header = [0u8; HEADER_SIZE];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }

    let length = u32::from_le_bytes(header[0..4].try_into().unwrap_or([0; 4])) as usize;
    let magic = u32::from_le_bytes(header[4..8].try_into().unwrap_or([0; 4]));
    let version = u32::from_le_bytes(header[8..12].try_into().unwrap_or([0; 4]));

    if magic != MAGIC {
        return Err(ProtocolError::BadMagic { found: magic });
    }
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::VersionMismatch {
            found: version,
            supported: PROTOCOL_VERSION,
        });
    }
    if length > MAX_MESSAGE_SIZE {
        return Err(ProtocolError::TooLarge {
            size: length,
            max: MAX_MESSAGE_SIZE,
        });
    }
    if length < HEADER_SIZE {
        return Err(ProtocolError::Malformed {
            detail: format!("frame length {length} is smaller than its own header"),
        });
    }

    let mut frame = Vec::with_capacity(length);
    frame.extend_from_slice(&header);
    frame.resize(length, 0);
    reader.read_exact(&mut frame[HEADER_SIZE..])?;
    Ok(Some(frame))
}

/// Write one frame to a stream and flush it.
pub fn write_frame<W: Write>(writer: &mut W, frame: &[u8]) -> Result<(), ProtocolError> {
    if frame.len() > MAX_MESSAGE_SIZE {
        return Err(ProtocolError::TooLarge {
            size: frame.len(),
            max: MAX_MESSAGE_SIZE,
        });
    }
    writer.write_all(frame)?;
    writer.flush()?;
    Ok(())
}

/// Convenience: encode, then write.
pub fn send<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<(), ProtocolError> {
    let frame = encode(value)?;
    write_frame(writer, &frame)
}

/// One volume as the service describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeSummary {
    /// Stable volume identifier.
    pub id: String,
    /// Current mount point label, for display.
    pub label: String,
    /// Sync status as a stable wire string.
    pub status: String,
    /// Live entries attributed to this volume.
    pub entries: usize,
    /// Journal cursor.
    pub cursor_usn: i64,
    /// Journal tip.
    pub next_usn: i64,
}

/// What a front end asks the service to do.
///
/// Every variant is a specific, bounded operation. There is intentionally no
/// way to express "run this", "open that" or "write somewhere".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "kebab-case")]
pub enum Request {
    /// Handshake. The client states the revision it speaks.
    Hello {
        /// Protocol revision the client implements.
        protocol_version: u32,
        /// Client name, for the service log.
        client: String,
    },
    /// Run a query against the service's index.
    Search {
        /// Query text: keywords, natural language or Local Search DSL.
        query: String,
        /// Entity type filter, empty for all.
        types: Vec<String>,
        /// Maximum hits.
        limit: usize,
    },
    /// Ask for index health.
    IndexStatus,
    /// Ask the service to rebuild one volume.
    RebuildVolume {
        /// Stable volume id, or an empty string for every volume.
        volume_id: String,
    },
    /// Poll for changes since a cursor.
    Changes {
        /// Only changes after this Unix millisecond timestamp.
        since_ms: i64,
        /// Maximum events to return.
        limit: usize,
    },
}

impl Request {
    /// A short name for logs.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Request::Hello { .. } => "hello",
            Request::Search { .. } => "search",
            Request::IndexStatus => "index-status",
            Request::RebuildVolume { .. } => "rebuild-volume",
            Request::Changes { .. } => "changes",
        }
    }
}

/// One filesystem change, as broadcast to subscribers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeEvent {
    /// Volume the change happened on.
    pub volume_id: String,
    /// `create`, `delete`, `rename` or `metadata`.
    pub kind: String,
    /// Affected path, when the service can resolve it.
    pub path: Option<String>,
    /// When the change was applied, in Unix milliseconds.
    pub at_ms: i64,
}

/// What the service answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "kebab-case")]
pub enum Response {
    /// Handshake reply.
    Hello {
        /// Protocol revision the service speaks.
        protocol_version: u32,
        /// Service build version.
        service_version: String,
        /// Whether the service is running with administrator rights.
        elevated: bool,
        /// Volumes the service is indexing.
        volumes: Vec<VolumeSummary>,
    },
    /// Search results.
    Search {
        /// Ranked results, exactly as the core produced them.
        ///
        /// The full `SearchResult` travels intact rather than a projection, so
        /// a client can act on a hit (open it, reveal it) without a second
        /// round trip or a lossy re-parse.
        results: Vec<search_core::SearchResult>,
        /// The compiled query, as the canonical Local Search DSL.
        compiled: String,
        /// Query plan name, when the service has one.
        plan: Option<String>,
        /// Service-side duration in milliseconds.
        elapsed_ms: f64,
        /// Total hits before truncation.
        total: usize,
        /// Whether the hit list is incomplete.
        truncated: bool,
    },
    /// Index health.
    IndexStatus {
        /// Service build version.
        service_version: String,
        /// Whether the index is ready to answer.
        ready: bool,
        /// Active backend.
        backend: String,
        /// Total indexed entries.
        entries: usize,
        /// Approximate memory footprint in bytes.
        memory_bytes: usize,
        /// Per-volume detail.
        volumes: Vec<VolumeSummary>,
    },
    /// A long-running job was accepted.
    Accepted {
        /// Job name.
        job: String,
    },
    /// Changes since the requested cursor.
    Changes {
        /// Events, oldest first.
        events: Vec<ChangeEvent>,
        /// Cursor to pass to the next call.
        next_cursor_ms: i64,
    },
    /// The request could not be served.
    Error {
        /// Stable error code.
        code: String,
        /// Human readable message.
        message: String,
    },
}

impl Response {
    /// A short name for logs.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Response::Hello { .. } => "hello",
            Response::Search { .. } => "search",
            Response::IndexStatus { .. } => "index-status",
            Response::Accepted { .. } => "accepted",
            Response::Changes { .. } => "changes",
            Response::Error { .. } => "error",
        }
    }

    /// Whether this response is an error.
    #[must_use]
    pub const fn is_error(&self) -> bool {
        matches!(self, Response::Error { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Request {
        Request::Hello {
            protocol_version: PROTOCOL_VERSION,
            client: "localsearch-cli/0.2.0".into(),
        }
    }

    #[test]
    fn a_request_round_trips_through_one_frame() {
        let frame = encode(&hello()).unwrap();
        assert_eq!(&frame[4..8], &MAGIC.to_le_bytes());
        assert_eq!(&frame[8..12], &PROTOCOL_VERSION.to_le_bytes());
        let decoded: Request = decode(&frame).unwrap();
        assert_eq!(decoded, hello());
    }

    #[test]
    fn every_request_shape_round_trips() {
        let requests = [
            hello(),
            Request::Search {
                query: "type:file ext:rs rust".into(),
                types: vec!["file".into()],
                limit: 25,
            },
            Request::IndexStatus,
            Request::RebuildVolume {
                volume_id: r"\\?\volume{abc}\".into(),
            },
            Request::Changes {
                since_ms: 1_700_000_000_000,
                limit: 100,
            },
        ];
        for request in requests {
            let frame = encode(&request).unwrap();
            let decoded: Request = decode(&frame).unwrap();
            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn every_response_shape_round_trips() {
        let responses = [
            Response::Hello {
                protocol_version: PROTOCOL_VERSION,
                service_version: "0.2.0".into(),
                elevated: true,
                volumes: vec![VolumeSummary {
                    id: "serial:00000001".into(),
                    label: "C:".into(),
                    status: "healthy".into(),
                    entries: 42,
                    cursor_usn: 100,
                    next_usn: 120,
                }],
            },
            Response::Search {
                results: vec![search_core::SearchResult::from_entity(
                    search_core::LocalEntity::File(search_core::FileEntry {
                        path: r"C:\a.rs".into(),
                        name: "a.rs".into(),
                        extension: Some("rs".into()),
                        size: 1_024,
                        modified: None,
                        created: None,
                        drive: Some('C'),
                        file_id: None,
                    }),
                    12.5,
                )],
                compiled: "type:file ext:rs".into(),
                plan: Some("trigram".into()),
                elapsed_ms: 0.8,
                total: 1,
                truncated: false,
            },
            Response::IndexStatus {
                service_version: "0.2.0".into(),
                ready: true,
                backend: "mft-usn".into(),
                entries: 1_000_000,
                memory_bytes: 70 * 1024 * 1024,
                volumes: Vec::new(),
            },
            Response::Accepted {
                job: "rebuild".into(),
            },
            Response::Changes {
                events: vec![ChangeEvent {
                    volume_id: "serial:00000001".into(),
                    kind: "create".into(),
                    path: Some(r"C:\new.txt".into()),
                    at_ms: 1,
                }],
                next_cursor_ms: 2,
            },
            Response::Error {
                code: "index-not-ready".into(),
                message: "still building".into(),
            },
        ];
        for response in responses {
            let frame = encode(&response).unwrap();
            let decoded: Response = decode(&frame).unwrap();
            assert_eq!(decoded, response);
        }
    }

    #[test]
    fn frames_can_be_streamed_and_read_back_in_order() {
        let mut buffer: Vec<u8> = Vec::new();
        send(&mut buffer, &hello()).unwrap();
        send(&mut buffer, &Request::IndexStatus).unwrap();

        let mut cursor = std::io::Cursor::new(buffer);
        let first: Request = decode(&read_frame(&mut cursor).unwrap().unwrap()).unwrap();
        let second: Request = decode(&read_frame(&mut cursor).unwrap().unwrap()).unwrap();
        assert_eq!(first, hello());
        assert_eq!(second, Request::IndexStatus);
        assert!(read_frame(&mut cursor).unwrap().is_none(), "clean end");
    }

    #[test]
    fn a_bad_magic_is_rejected() {
        let mut frame = encode(&hello()).unwrap();
        frame[4..8].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        assert!(matches!(
            decode::<Request>(&frame),
            Err(ProtocolError::BadMagic { .. })
        ));
    }

    #[test]
    fn a_wrong_version_is_rejected_rather_than_guessed_at() {
        let mut frame = encode(&hello()).unwrap();
        frame[8..12].copy_from_slice(&(PROTOCOL_VERSION + 1).to_le_bytes());
        assert!(matches!(
            decode::<Request>(&frame),
            Err(ProtocolError::VersionMismatch { .. })
        ));
    }

    #[test]
    fn an_oversized_frame_is_rejected_before_allocating() {
        let mut frame = encode(&hello()).unwrap();
        let claimed = (MAX_MESSAGE_SIZE + 1) as u32;
        frame[0..4].copy_from_slice(&claimed.to_le_bytes());
        assert!(matches!(
            decode::<Request>(&frame),
            Err(ProtocolError::TooLarge { .. })
        ));

        // And the streaming reader refuses to allocate the body.
        let mut cursor = std::io::Cursor::new(frame);
        assert!(matches!(
            read_frame(&mut cursor),
            Err(ProtocolError::TooLarge { .. })
        ));
    }

    #[test]
    fn a_truncated_frame_is_an_error_not_an_empty_message() {
        let frame = encode(&hello()).unwrap();
        let cut = &frame[..frame.len() - 3];
        assert!(matches!(
            decode::<Request>(cut),
            Err(ProtocolError::Truncated)
        ));

        let mut cursor = std::io::Cursor::new(cut.to_vec());
        assert!(matches!(
            read_frame(&mut cursor),
            Err(ProtocolError::Io(_)) | Err(ProtocolError::Truncated)
        ));
    }

    #[test]
    fn a_frame_shorter_than_its_own_header_is_malformed() {
        let mut frame = encode(&hello()).unwrap();
        frame[0..4].copy_from_slice(&4u32.to_le_bytes());
        let mut cursor = std::io::Cursor::new(frame);
        assert!(matches!(
            read_frame(&mut cursor),
            Err(ProtocolError::Malformed { .. })
        ));
    }

    #[test]
    fn a_malformed_body_is_reported_as_malformed() {
        let mut frame = encode(&hello()).unwrap();
        frame.truncate(HEADER_SIZE + 2);
        frame[0..4].copy_from_slice(&((HEADER_SIZE + 2) as u32).to_le_bytes());
        assert!(matches!(
            decode::<Request>(&frame),
            Err(ProtocolError::Malformed { .. })
        ));
    }

    #[test]
    fn encoding_something_absurd_is_refused() {
        // A request whose payload is far beyond the frame limit.
        let huge = Request::Search {
            query: "x".repeat(MAX_MESSAGE_SIZE),
            types: Vec::new(),
            limit: 1,
        };
        assert!(matches!(encode(&huge), Err(ProtocolError::TooLarge { .. })));
    }

    #[test]
    fn the_protocol_cannot_express_code_execution() {
        // This test is a compile-time assertion as much as a runtime one: the
        // message shapes below are the complete set, and none of them can run
        // a command. If a future variant tried to, this match would stop
        // compiling and the reviewer would see it.
        let request = Request::Search {
            query: "anything".into(),
            types: Vec::new(),
            limit: 1,
        };
        match request {
            Request::Hello { .. }
            | Request::Search { .. }
            | Request::IndexStatus
            | Request::RebuildVolume { .. }
            | Request::Changes { .. } => {}
        }
        assert_eq!(Request::IndexStatus.kind(), "index-status");
    }

    #[test]
    fn kinds_are_stable() {
        assert_eq!(hello().kind(), "hello");
        assert_eq!(
            Response::IndexStatus {
                service_version: String::new(),
                ready: false,
                backend: String::new(),
                entries: 0,
                memory_bytes: 0,
                volumes: Vec::new(),
            }
            .kind(),
            "index-status"
        );
        assert!(Response::Error {
            code: "x".into(),
            message: "y".into()
        }
        .is_error());
    }

    #[test]
    fn the_default_pipe_name_is_versioned() {
        assert!(DEFAULT_PIPE_NAME.contains("localsearch"));
        assert!(DEFAULT_PIPE_NAME.ends_with("-v1"));
    }
}
