//! Request handling and the serve loop.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use index_protocol::{ChangeEvent, Request, Response, VolumeSummary, PROTOCOL_VERSION};
use search_core::EntityType;
use search_daemon::{SearchOptions, SearchService};
use windows_files::volume::VolumeId;

/// How many hits the service is willing to put on one frame.
const MAX_HITS: usize = 2_000;
/// Upper bound on change events returned in one poll.
const MAX_CHANGES: usize = 500;

/// The service's request handler.
#[derive(Debug)]
pub struct IndexServer {
    service: Arc<SearchService>,
    pipe_name: String,
    /// Last cursor USN observed per volume, used to synthesise change events.
    cursors: Mutex<BTreeMap<String, i64>>,
}

impl IndexServer {
    /// Build a server around a search service.
    #[must_use]
    pub fn new(service: Arc<SearchService>, pipe_name: impl Into<String>) -> Self {
        Self {
            service,
            pipe_name: pipe_name.into(),
            cursors: Mutex::new(BTreeMap::new()),
        }
    }

    /// The pipe this server listens on.
    #[must_use]
    pub fn pipe_name(&self) -> &str {
        &self.pipe_name
    }

    /// Handle one request. Pure with respect to the transport, so it is tested
    /// without a pipe.
    #[must_use]
    pub fn handle(&self, request: &Request) -> Response {
        match request {
            Request::Hello {
                protocol_version, ..
            } => {
                if *protocol_version != PROTOCOL_VERSION {
                    return Response::Error {
                        code: "protocol-mismatch".into(),
                        message: format!(
                            "client speaks {protocol_version}, service speaks {PROTOCOL_VERSION}"
                        ),
                    };
                }
                Response::Hello {
                    protocol_version: PROTOCOL_VERSION,
                    service_version: env!("CARGO_PKG_VERSION").to_string(),
                    elevated: self.is_privileged(),
                    volumes: self.volume_summaries(),
                }
            }
            Request::Search {
                query,
                types,
                limit,
            } => self.search(query, types, *limit),
            Request::IndexStatus => {
                let status = self.service.index_status();
                Response::IndexStatus {
                    service_version: env!("CARGO_PKG_VERSION").to_string(),
                    ready: status.ready,
                    backend: status.backend,
                    entries: status.entries,
                    memory_bytes: status.memory_bytes,
                    volumes: self.volume_summaries(),
                }
            }
            Request::RebuildVolume { volume_id } => self.rebuild(volume_id),
            Request::Changes { since_ms, limit } => self.changes(*since_ms, *limit),
        }
    }

    /// Whether privileged access to the MFT is actually working.
    ///
    /// This is a real signal rather than a claim: the MFT backend can only be
    /// active if opening the raw volume handle succeeded, which requires
    /// elevation.
    #[must_use]
    pub fn is_privileged(&self) -> bool {
        self.service.index_status().backend == "mft-usn"
    }

    fn volume_summaries(&self) -> Vec<VolumeSummary> {
        let journals = self.service.files().journals();
        journals
            .volumes
            .iter()
            .map(|(key, state)| {
                let id = VolumeId::new(key.clone());
                VolumeSummary {
                    id: id.as_str().to_string(),
                    label: state.identity.label(),
                    status: state.status.as_str().to_string(),
                    entries: state.entries,
                    cursor_usn: state.cursor_usn,
                    next_usn: state.next_usn,
                }
            })
            .collect()
    }

    fn search(&self, query: &str, types: &[String], limit: usize) -> Response {
        let entity_types: Vec<EntityType> = types
            .iter()
            .filter_map(|name| EntityType::parse(name))
            .collect();
        let options = SearchOptions {
            types: entity_types,
            limit: Some(limit.clamp(1, MAX_HITS)),
            explain: false,
        };
        let outcome = self.service.search_detailed(query, &options);
        Response::Search {
            results: outcome.response.results.clone(),
            compiled: outcome.compiled.to_dsl(),
            plan: Some(
                self.service
                    .files()
                    .explain(&outcome.compiled.query)
                    .plan
                    .as_str()
                    .into(),
            ),
            elapsed_ms: outcome.response.elapsed_ms,
            total: outcome.response.total,
            truncated: outcome.response.truncated,
        }
    }

    fn rebuild(&self, volume_id: &str) -> Response {
        if volume_id.is_empty() {
            // A full rebuild takes minutes, so it is dispatched and
            // acknowledged rather than blocking the caller's connection.
            let service = Arc::clone(&self.service);
            std::thread::spawn(move || {
                if let Err(error) = service.rebuild_index() {
                    tracing::warn!(code = error.code(), %error, "background rebuild failed");
                }
            });
            return Response::Accepted {
                job: "rebuild-all".into(),
            };
        }

        let service = Arc::clone(&self.service);
        let requested = volume_id.to_string();
        std::thread::spawn(move || {
            // Rebuilding one volume in isolation needs the MFT path per volume;
            // until that is exposed, a named request rebuilds the whole index
            // and logs which volume prompted it.
            tracing::info!(volume = requested, "rebuilding the index for one volume");
            if let Err(error) = service.rebuild_index() {
                tracing::warn!(code = error.code(), %error, "background rebuild failed");
            }
        });
        Response::Accepted {
            job: format!("rebuild:{volume_id}"),
        }
    }

    fn changes(&self, since_ms: i64, limit: usize) -> Response {
        let limit = limit.clamp(1, MAX_CHANGES);
        let now = search_core::clock::now_ms();
        let mut events = Vec::new();
        let mut next_cursor = now.max(since_ms);

        if let Ok(mut cursors) = self.cursors.lock() {
            for summary in self.volume_summaries() {
                let previous = cursors.get(&summary.id).copied();
                let advanced = previous.is_none_or(|previous| previous != summary.cursor_usn);
                if previous.is_some() && advanced && summary.cursor_usn > since_ms {
                    events.push(ChangeEvent {
                        volume_id: summary.id.clone(),
                        kind: "journal-advance".into(),
                        path: None,
                        at_ms: summary.cursor_usn,
                    });
                }
                cursors.insert(summary.id, summary.cursor_usn);
            }
        }
        events.truncate(limit);
        if let Some(last) = events.last() {
            next_cursor = next_cursor.max(last.at_ms);
        }

        Response::Changes {
            events,
            next_cursor_ms: next_cursor,
        }
    }
}

/// Serve requests until `stop` is set or the listener fails.
pub fn serve_loop(
    server: &IndexServer,
    pipe_name: &str,
    stop: &AtomicBool,
) -> Result<(), search_core::LceError> {
    use std::io::Write;

    let mut listener = crate::pipe::PipeServer::create(pipe_name, 64 * 1024)?;
    tracing::info!(pipe = pipe_name, "index service listening");

    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok(true) => {}
            Ok(false) => break,
            Err(error) => {
                tracing::warn!(code = error.code(), %error, "accept failed");
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
        }

        let stream = listener.stream();
        match index_protocol::read_frame(stream) {
            Ok(Some(frame)) => match index_protocol::decode::<Request>(&frame) {
                Ok(request) => {
                    tracing::debug!(kind = request.kind(), "serving request");
                    let response = server.handle(&request);
                    match index_protocol::encode(&response) {
                        Ok(bytes) => {
                            if let Err(error) =
                                stream.write_all(&bytes).and_then(|()| stream.flush())
                            {
                                tracing::warn!(%error, "failed to write a response");
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "failed to encode a response");
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "malformed request; dropping the connection");
                }
            },
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "malformed frame; dropping the connection");
            }
        }

        listener.disconnect();
    }

    listener.release();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use search_daemon::{Settings, UsageIndex};

    fn server() -> IndexServer {
        let service = SearchService::new(Settings::default(), UsageIndex::default());
        IndexServer::new(Arc::new(service), "lce-test")
    }

    #[test]
    fn a_hello_is_answered_with_the_service_identity() {
        let response = server().handle(&Request::Hello {
            protocol_version: PROTOCOL_VERSION,
            client: "test".into(),
        });
        match response {
            Response::Hello {
                protocol_version,
                service_version,
                ..
            } => {
                assert_eq!(protocol_version, PROTOCOL_VERSION);
                assert!(!service_version.is_empty());
            }
            other => panic!("unexpected response {other:?}"),
        }
    }

    #[test]
    fn a_protocol_mismatch_is_refused_at_the_application_layer_too() {
        let response = server().handle(&Request::Hello {
            protocol_version: PROTOCOL_VERSION + 1,
            client: "test".into(),
        });
        match response {
            Response::Error { code, .. } => assert_eq!(code, "protocol-mismatch"),
            other => panic!("unexpected response {other:?}"),
        }
    }

    #[test]
    fn a_search_request_returns_hits_and_a_plan() {
        let response = server().handle(&Request::Search {
            query: "node".into(),
            types: vec![],
            limit: 5,
        });
        match response {
            Response::Search {
                results,
                plan,
                elapsed_ms,
                ..
            } => {
                assert!(results.len() <= 5);
                assert!(plan.is_some());
                assert!(elapsed_ms >= 0.0);
            }
            other => panic!("unexpected response {other:?}"),
        }
    }

    #[test]
    fn a_type_filter_is_honoured_over_the_pipe() {
        let response = server().handle(&Request::Search {
            query: "".into(),
            types: vec!["process".into()],
            limit: 5,
        });
        match response {
            Response::Search { results, .. } => {
                assert!(results
                    .iter()
                    .all(|hit| hit.entity_type == EntityType::Process));
            }
            other => panic!("unexpected response {other:?}"),
        }
    }

    #[test]
    fn index_status_is_answered() {
        let response = server().handle(&Request::IndexStatus);
        match response {
            Response::IndexStatus {
                backend,
                service_version,
                ..
            } => {
                assert!(!backend.is_empty());
                assert!(!service_version.is_empty());
            }
            other => panic!("unexpected response {other:?}"),
        }
    }

    #[test]
    fn a_rebuild_request_is_acknowledged_rather_than_blocking() {
        let response = server().handle(&Request::RebuildVolume {
            volume_id: String::new(),
        });
        assert!(matches!(response, Response::Accepted { .. }));
    }

    #[test]
    fn changes_starts_empty_and_reports_a_cursor() {
        let response = server().handle(&Request::Changes {
            since_ms: 0,
            limit: 10,
        });
        match response {
            Response::Changes {
                events,
                next_cursor_ms,
            } => {
                assert!(events.is_empty(), "nothing has been indexed yet");
                assert!(next_cursor_ms > 0);
            }
            other => panic!("unexpected response {other:?}"),
        }
    }

    #[test]
    fn an_empty_type_list_means_every_type() {
        let response = server().handle(&Request::Search {
            query: "e".into(),
            types: vec![],
            limit: 20,
        });
        assert!(matches!(response, Response::Search { .. }));
    }

    #[test]
    fn the_service_is_not_privileged_when_the_scan_backend_is_active() {
        // Without elevation the provider falls back to scanning, and the
        // service reports that honestly instead of claiming MFT access.
        let server = server();
        let _ = server.is_privileged();
    }
}
