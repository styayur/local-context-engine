//! Structured errors.
//!
//! The rule is simple: the user interface never renders a raw OS error. Every
//! failure carries a stable machine code (which the UI translates) plus the
//! original developer-facing detail (which only the log shows).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Something went wrong while running a search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum SearchError {
    /// The query could not be understood at all.
    #[error("invalid query: {reason}")]
    InvalidQuery {
        /// What was wrong with the query.
        reason: String,
    },
    /// A provider returned an unusable record.
    #[error("provider `{provider}` returned a malformed record: {detail}")]
    MalformedRecord {
        /// Name of the provider.
        provider: String,
        /// What was wrong.
        detail: String,
    },
    /// The search was cancelled by the caller.
    #[error("search cancelled")]
    Cancelled,
}

/// Something went wrong while building or updating an index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum IndexError {
    /// The index cache file is unreadable or in an unsupported format.
    #[error("index cache is not readable: {path}")]
    CacheUnreadable {
        /// Path of the cache file.
        path: String,
    },
    /// The volume does not expose an NTFS change journal.
    #[error("volume {volume} has no USN journal")]
    JournalUnavailable {
        /// Volume identifier, for example `C:`.
        volume: String,
    },
    /// The volume is not NTFS, so the MFT backend cannot be used.
    #[error("volume {volume} is not NTFS")]
    NotNtfs {
        /// Volume identifier, for example `C:`.
        volume: String,
    },
    /// The index is still being built.
    #[error("index for {volume} is not ready yet")]
    NotReady {
        /// Volume identifier, for example `C:`.
        volume: String,
    },
    /// Generic backend failure.
    #[error("index backend `{backend}` failed: {detail}")]
    Backend {
        /// Which backend failed, for example `mft-usn` or `scan`.
        backend: String,
        /// Developer-facing detail.
        detail: String,
    },
}

/// Something went wrong in a platform (Win32) call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum PlatformError {
    /// A Win32 call returned an error code.
    #[error("windows api `{call}` failed with code {code}")]
    WindowsApi {
        /// The API that failed.
        call: String,
        /// The raw `GetLastError`/HRESULT value.
        code: i64,
    },
    /// A feature is not available on this build of Windows.
    #[error("`{feature}` is not available on this system")]
    Unsupported {
        /// The feature that is unavailable.
        feature: String,
    },
    /// A handle could not be opened or a resource was missing.
    #[error("`{resource}` is not available")]
    NotFound {
        /// The resource that was missing.
        resource: String,
    },
}

/// The caller is not allowed to do what it asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum PermissionError {
    /// Opening a raw volume handle was denied (the common non-elevated case).
    #[error("access to volume {volume} was denied")]
    VolumeAccessDenied {
        /// Volume identifier, for example `C:`.
        volume: String,
    },
    /// A generic access-denied on a specific operation.
    #[error("access denied while {action}")]
    AccessDenied {
        /// What the caller tried to do.
        action: String,
    },
    /// The operation needs an elevated process.
    #[error("administrator privileges are required to {reason}")]
    ElevationRequired {
        /// Why elevation is needed.
        reason: String,
    },
}

/// The unified error type used across every crate in the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum LceError {
    /// A search-time failure.
    #[error(transparent)]
    Search(#[from] SearchError),
    /// An index-time failure.
    #[error(transparent)]
    Index(#[from] IndexError),
    /// A platform failure.
    #[error(transparent)]
    Platform(#[from] PlatformError),
    /// A permissions failure.
    #[error(transparent)]
    Permission(#[from] PermissionError),
    /// Local I/O that is not covered by the variants above.
    #[error("io error while {action}: {detail}")]
    Io {
        /// What the caller tried to do.
        action: String,
        /// Developer-facing detail.
        detail: String,
    },
}

impl LceError {
    /// Build an [`LceError::Io`] from a `std::io::Error`.
    #[must_use]
    pub fn io(action: impl Into<String>, error: &std::io::Error) -> Self {
        LceError::Io {
            action: action.into(),
            detail: error.to_string(),
        }
    }

    /// Stable machine-readable code.
    ///
    /// The desktop UI maps these to translation keys; the CLI prints them in
    /// `--json` mode so scripts can branch on them.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            LceError::Search(SearchError::InvalidQuery { .. }) => "invalid-query",
            LceError::Search(SearchError::MalformedRecord { .. }) => "malformed-record",
            LceError::Search(SearchError::Cancelled) => "cancelled",
            LceError::Index(IndexError::CacheUnreadable { .. }) => "index-cache-unreadable",
            LceError::Index(IndexError::JournalUnavailable { .. }) => "index-journal-unavailable",
            LceError::Index(IndexError::NotNtfs { .. }) => "index-not-ntfs",
            LceError::Index(IndexError::NotReady { .. }) => "index-not-ready",
            LceError::Index(IndexError::Backend { .. }) => "index-backend-failed",
            LceError::Platform(PlatformError::WindowsApi { .. }) => "windows-api-failed",
            LceError::Platform(PlatformError::Unsupported { .. }) => "unsupported",
            LceError::Platform(PlatformError::NotFound { .. }) => "not-found",
            LceError::Permission(PermissionError::VolumeAccessDenied { .. }) => {
                "volume-access-denied"
            }
            LceError::Permission(PermissionError::AccessDenied { .. }) => "access-denied",
            LceError::Permission(PermissionError::ElevationRequired { .. }) => "elevation-required",
            LceError::Io { .. } => "io-error",
        }
    }

    /// Translation key the UI should look up.
    #[must_use]
    pub fn message_key(&self) -> String {
        format!("error.{}", self.code())
    }

    /// A short, actionable hint for the user, in English.
    ///
    /// The desktop UI prefers its own localised copy keyed off
    /// [`LceError::code`]; this is the fallback for the CLI and MCP server.
    #[must_use]
    pub const fn hint(&self) -> &'static str {
        match self {
            LceError::Permission(PermissionError::VolumeAccessDenied { .. }) => {
                "Unable to access this volume. Administrator privileges may be required."
            }
            LceError::Permission(PermissionError::ElevationRequired { .. }) => {
                "Run Local Context Engine as administrator to index this volume, or keep using the fallback file backend."
            }
            LceError::Index(IndexError::NotNtfs { .. }) => {
                "Only NTFS volumes support the MFT backend; the directory scan backend will be used instead."
            }
            LceError::Index(IndexError::JournalUnavailable { .. }) => {
                "The NTFS change journal is disabled or full. Run `fsutil usn createjournal` as administrator to enable incremental updates."
            }
            LceError::Index(IndexError::CacheUnreadable { .. }) => {
                "The cached index will be rebuilt from scratch."
            }
            LceError::Index(IndexError::NotReady { .. }) => {
                "The index is still being built. Results will improve as it fills in."
            }
            LceError::Platform(PlatformError::WindowsApi { .. }) => {
                "The system call failed. See the developer log for the raw error."
            }
            LceError::Platform(PlatformError::Unsupported { .. }) => {
                "This Windows build does not expose the required API."
            }
            LceError::Platform(PlatformError::NotFound { .. }) => {
                "The requested resource no longer exists."
            }
            LceError::Search(SearchError::InvalidQuery { .. }) => {
                "The query could not be parsed; it was treated as plain keywords."
            }
            LceError::Search(SearchError::MalformedRecord { .. }) => {
                "A data source returned an unusable record; it was skipped."
            }
            LceError::Search(SearchError::Cancelled) => "The search was cancelled.",
            LceError::Index(IndexError::Backend { .. }) => {
                "The index backend reported a failure; the fallback backend will be used."
            }
            LceError::Permission(PermissionError::AccessDenied { .. }) => {
                "Access was denied. Administrator privileges may be required."
            }
            LceError::Io { .. } => "A local file operation failed.",
        }
    }

    /// Whether the failure is recoverable by simply continuing.
    #[must_use]
    pub const fn is_recoverable(&self) -> bool {
        matches!(
            self,
            LceError::Permission(_)
                | LceError::Index(_)
                | LceError::Platform(_)
                | LceError::Search(SearchError::MalformedRecord { .. })
        )
    }
}

impl From<std::io::Error> for LceError {
    fn from(error: std::io::Error) -> Self {
        LceError::Io {
            action: "performing local io".into(),
            detail: error.to_string(),
        }
    }
}

/// Convenience alias used by providers.
pub type Result<T, E = LceError> = std::result::Result<T, E>;

/// A list of recoverable problems collected while running a search.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WarningList(pub Vec<String>);

impl WarningList {
    /// Record a warning.
    pub fn push(&mut self, warning: impl Into<String>) {
        self.0.push(warning.into());
    }

    /// Borrow the raw warnings.
    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    /// Consume the list.
    #[must_use]
    pub fn into_vec(self) -> Vec<String> {
        self.0
    }

    /// Whether nothing went wrong.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for WarningList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, warning) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            f.write_str(warning)?;
        }
        Ok(())
    }
}
