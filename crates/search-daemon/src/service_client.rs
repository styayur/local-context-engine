//! Talking to the privileged index service from an unprivileged front end.
//!
//! When `lce-index-service.exe` is installed and running, this is how the CLI,
//! the MCP server and the desktop shell ask it questions. When it is not, the
//! caller falls back to its own in-process provider — a missing service is a
//! state to report, never a crash.

use index_client::IndexClient;
use index_protocol::{Request, Response, DEFAULT_PIPE_NAME, PROTOCOL_VERSION};
use search_core::{EntityType, LceError, SearchResponse};

/// What the local index service is doing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    /// Whether a connection could be made right now.
    pub available: bool,
    /// Pipe the client looks for.
    pub pipe: String,
    /// Protocol revision this build speaks.
    pub protocol_version: u32,
    /// Service build version, when reachable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_version: Option<String>,
    /// Whether the service reports working privileged access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevated: Option<bool>,
    /// Volume count reported by the service.
    pub volumes: usize,
}

impl ServiceStatus {
    /// A status for a service that is not reachable.
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            available: false,
            pipe: DEFAULT_PIPE_NAME.to_string(),
            protocol_version: PROTOCOL_VERSION,
            service_version: None,
            elevated: None,
            volumes: 0,
        }
    }

    /// A one-line summary for the UI.
    #[must_use]
    pub fn describe(&self) -> String {
        if !self.available {
            return format!("not running (pipe {})", self.pipe);
        }
        let privileged = match self.elevated {
            Some(true) => "privileged",
            Some(false) => "unprivileged",
            None => "unknown privileges",
        };
        format!(
            "running {} — {privileged}, {} volume(s)",
            self.service_version.as_deref().unwrap_or("version unknown"),
            self.volumes
        )
    }
}

/// Whether the index service is reachable.
#[must_use]
pub fn is_available() -> bool {
    IndexClient::is_available()
}

/// Ask the service for its status.
#[must_use]
pub fn status() -> ServiceStatus {
    let mut client = match IndexClient::connect() {
        Ok(client) => client,
        Err(_) => return ServiceStatus::unavailable(),
    };
    match client.hello() {
        Ok(Response::Hello {
            protocol_version,
            service_version,
            elevated,
            volumes,
        }) => ServiceStatus {
            available: true,
            pipe: client.pipe_name().to_string(),
            protocol_version,
            service_version: Some(service_version),
            elevated: Some(elevated),
            volumes: volumes.len(),
        },
        _ => ServiceStatus {
            available: true,
            pipe: client.pipe_name().to_string(),
            ..ServiceStatus::unavailable()
        },
    }
}

/// Run a search through the service.
///
/// Returns `Ok(None)` when the service is not running, which the caller should
/// treat as "use the local provider" rather than as a failure.
pub fn search(
    query: &str,
    types: &[EntityType],
    limit: usize,
) -> Result<Option<SearchResponse>, LceError> {
    let Ok(mut client) = IndexClient::connect() else {
        return Ok(None);
    };
    client.handshake()?;

    let response = client.request(&Request::Search {
        query: query.to_string(),
        types: types
            .iter()
            .map(|entity_type| entity_type.as_str().to_string())
            .collect(),
        limit,
    })?;

    match response {
        Response::Search {
            results,
            compiled,
            plan: _,
            elapsed_ms,
            total,
            truncated,
        } => Ok(Some(SearchResponse {
            query: query.to_string(),
            compiled,
            elapsed_ms,
            total,
            truncated,
            results,
            timings: Vec::new(),
            warnings: Vec::new(),
        })),
        Response::Error { code, message } => Err(LceError::Io {
            action: "searching through the index service".into(),
            detail: format!("{code}: {message}"),
        }),
        other => Err(LceError::Io {
            action: "searching through the index service".into(),
            detail: format!("unexpected response `{}`", other.kind()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unavailable_status_describes_itself() {
        let status = ServiceStatus::unavailable();
        assert!(!status.available);
        assert_eq!(status.pipe, DEFAULT_PIPE_NAME);
        assert!(status.describe().contains("not running"));
    }

    #[test]
    fn a_running_status_describes_its_privileges() {
        let status = ServiceStatus {
            available: true,
            pipe: DEFAULT_PIPE_NAME.into(),
            protocol_version: PROTOCOL_VERSION,
            service_version: Some("0.2.0".into()),
            elevated: Some(true),
            volumes: 2,
        };
        let described = status.describe();
        assert!(described.contains("privileged"), "{described}");
        assert!(described.contains("2 volume"), "{described}");
        assert!(!described.contains("unprivileged"));
    }

    #[test]
    fn status_never_panics_when_the_service_is_absent() {
        // Whatever this machine has installed, asking must be safe.
        let status = status();
        assert!(!status.pipe.is_empty());
    }

    #[test]
    fn searching_without_a_service_is_not_an_error() {
        // It reports "no service", which the caller turns into a fallback.
        match search("x", &[], 5) {
            Ok(None) => {}
            Ok(Some(response)) => assert!(response.results.len() <= 5),
            Err(error) => panic!("unexpected error: {error}"),
        }
    }
}
