//! The privileged half of the IPC boundary.
//!
//! `lce-index-service.exe` is the only component that runs elevated. Its job
//! list is deliberately tiny:
//!
//! * own the NTFS MFT enumeration and the per-volume USN journals;
//! * answer typed, versioned requests from the unprivileged front ends over a
//!   Named Pipe whose DACL it builds explicitly;
//! * report index health.
//!
//! It cannot launch a program, open a document, terminate a process or write a
//! file — those operations are not expressible in [`index_protocol::Request`],
//! so no client can ask for them even if it wanted to.
//!
//! # Trust boundary
//!
//! MFT enumeration can reveal file *names* the calling user could not discover
//! by walking the directory tree, because the MFT does not apply per-directory
//! ACLs. That is a real, deliberate property of the design, and it is why:
//!
//! * the pipe grants access only to the interactive user that installed the
//!   service, plus SYSTEM and Administrators;
//! * remote clients are rejected by the pipe itself
//!   (`PIPE_REJECT_REMOTE_CLIENTS`), not merely by the ACL;
//! * the service answers index queries and nothing else.
//!
//! See `docs/SECURITY.md` for the full threat model.

use std::sync::atomic::{AtomicBool, Ordering};

use index_protocol::{Request, Response, DEFAULT_PIPE_NAME};

pub mod pipe;
pub mod server;

/// Set when the service control manager asks the process to stop.
static SHOULD_STOP: AtomicBool = AtomicBool::new(false);

/// Ask the running pipe server to stop after the current request.
pub fn request_stop() {
    SHOULD_STOP.store(true, Ordering::SeqCst);
}

/// Whether a stop has been requested.
#[must_use]
pub fn stop_requested() -> bool {
    SHOULD_STOP.load(Ordering::SeqCst)
}

/// Clear the stop flag, so a restarted server is not instantly shut down.
pub fn clear_stop() {
    SHOULD_STOP.store(false, Ordering::SeqCst);
}

/// The default pipe this build serves.
#[must_use]
pub fn default_pipe_name() -> &'static str {
    DEFAULT_PIPE_NAME
}

/// Turn a request into a response.
///
/// Kept separate from the transport so it can be unit tested without a pipe at
/// all.
#[must_use]
pub fn describe_request(request: &Request) -> &'static str {
    request.kind()
}

/// A response is an error when the code is set.
#[must_use]
pub fn is_error_response(response: &Response) -> bool {
    response.is_error()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stop_flag_round_trips() {
        clear_stop();
        assert!(!stop_requested());
        request_stop();
        assert!(stop_requested());
        clear_stop();
        assert!(!stop_requested());
    }

    #[test]
    fn the_default_pipe_is_the_protocol_default() {
        assert_eq!(default_pipe_name(), DEFAULT_PIPE_NAME);
    }

    #[test]
    fn requests_are_described_by_kind() {
        assert_eq!(describe_request(&Request::IndexStatus), "index-status");
        assert_eq!(
            describe_request(&Request::Hello {
                protocol_version: index_protocol::PROTOCOL_VERSION,
                client: "test".into(),
            }),
            "hello"
        );
    }

    #[test]
    fn a_client_and_server_exchange_a_handshake_over_a_real_pipe() {
        use std::sync::{Arc, Mutex};

        let pipe = format!("lce-ipc-test-{}", std::process::id());
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let server_pipe = pipe.clone();

        let reported: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let server_reported = Arc::clone(&reported);
        let handle = std::thread::spawn(move || {
            let service = Arc::new(search_daemon::SearchService::new(
                search_daemon::Settings::default(),
                search_daemon::UsageIndex::default(),
            ));
            let server = server::IndexServer::new(service, server_pipe.clone());
            if let Err(error) = server::serve_loop(&server, &server_pipe, &server_stop) {
                if let Ok(mut guard) = server_reported.lock() {
                    *guard = Some(format!("{} ({})", error.hint(), error.code()));
                }
            }
        });

        // Give the server a moment to create the pipe before connecting.
        let mut client = None;
        for _ in 0..50 {
            if let Ok(candidate) = index_client::IndexClient::connect_to(&pipe) {
                client = Some(candidate);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let mut client = match client {
            Some(client) => client,
            None => {
                let reason = reported
                    .lock()
                    .ok()
                    .and_then(|guard| guard.clone())
                    .unwrap_or_else(|| "the server did not report a reason".into());
                stop.store(true, Ordering::SeqCst);
                let _ = handle.join();
                panic!("the pipe server never came up: {reason}");
            }
        };

        let hello = client.handshake().expect("handshake");
        match hello {
            Response::Hello {
                protocol_version, ..
            } => assert_eq!(protocol_version, index_protocol::PROTOCOL_VERSION),
            other => panic!("unexpected handshake response: {other:?}"),
        }

        stop.store(true, Ordering::SeqCst);
        // Nudge the server out of AcceptNamedPipe so it observes the stop flag.
        let _ = index_client::IndexClient::connect_to(&pipe);
        let _ = handle.join();
    }

    #[test]
    fn error_responses_are_recognised() {
        assert!(is_error_response(&Response::Error {
            code: "x".into(),
            message: "y".into(),
        }));
        assert!(!is_error_response(&Response::Accepted { job: "j".into() }));
    }
}
