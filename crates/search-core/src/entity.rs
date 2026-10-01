//! The unified local entity model.
//!
//! Every data source in the engine (filesystem, process table, Start Menu,
//! service control manager, window manager) is normalised into a
//! [`LocalEntity`] variant so that ranking, filtering and presentation only
//! ever have to deal with one shape.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// The six searchable domains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityType {
    File,
    Directory,
    Process,
    Application,
    Service,
    Window,
}

impl EntityType {
    /// Every variant, in the order shown in the UI type filter.
    pub const ALL: [EntityType; 6] = [
        EntityType::File,
        EntityType::Directory,
        EntityType::Process,
        EntityType::Application,
        EntityType::Service,
        EntityType::Window,
    ];

    /// Lower-case wire name (also the value used in `type:` DSL filters).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            EntityType::File => "file",
            EntityType::Directory => "directory",
            EntityType::Process => "process",
            EntityType::Application => "application",
            EntityType::Service => "service",
            EntityType::Window => "window",
        }
    }

    /// Human readable label used when no translation is available.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            EntityType::File => "File",
            EntityType::Directory => "Folder",
            EntityType::Process => "Process",
            EntityType::Application => "Application",
            EntityType::Service => "Service",
            EntityType::Window => "Window",
        }
    }

    /// Parse a `type:` token. Accepts a few friendly aliases so that both
    /// `type:folder` and `type:directory` work.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let value = raw.trim().to_ascii_lowercase();
        Some(match value.as_str() {
            "file" | "files" => EntityType::File,
            "dir" | "directory" | "directories" | "folder" | "folders" => EntityType::Directory,
            "process" | "processes" | "proc" | "ps" => EntityType::Process,
            "app" | "apps" | "application" | "applications" | "program" | "programs" => {
                EntityType::Application
            }
            "service" | "services" | "svc" => EntityType::Service,
            "window" | "windows" | "win" => EntityType::Window,
            _ => return None,
        })
    }
}

impl fmt::Display for EntityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A file on any indexed volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Fully qualified path, using native separators.
    pub path: String,
    /// File name including the extension.
    pub name: String,
    /// Lower-case extension without the leading dot, when there is one.
    pub extension: Option<String>,
    /// Size in bytes. Directories report `0`.
    pub size: u64,
    /// Last write time as Unix milliseconds.
    pub modified: Option<i64>,
    /// Creation time as Unix milliseconds.
    pub created: Option<i64>,
    /// Drive letter the file lives on, when it is a simple volume path.
    pub drive: Option<char>,
    /// NTFS file reference number, when the backend knows it.
    pub file_id: Option<u64>,
}

/// A directory on any indexed volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryEntry {
    /// Fully qualified path, using native separators.
    pub path: String,
    /// Directory name (the last path component).
    pub name: String,
    /// Last write time as Unix milliseconds.
    pub modified: Option<i64>,
    /// Creation time as Unix milliseconds.
    pub created: Option<i64>,
    /// Drive letter the directory lives on, when it is a simple volume path.
    pub drive: Option<char>,
    /// NTFS file reference number, when the backend knows it.
    pub file_id: Option<u64>,
}

/// A live process from the system process table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessEntry {
    /// Process identifier.
    pub pid: u32,
    /// Image name, for example `Code.exe`.
    pub name: String,
    /// Full image path when it can be read.
    pub exe_path: Option<String>,
    /// Creating process id, when reported.
    pub parent_pid: Option<u32>,
    /// Working set size in bytes.
    pub memory_bytes: u64,
    /// Process start time as Unix milliseconds.
    pub start_time: Option<i64>,
    /// Owning account, when readable without elevation.
    pub username: Option<String>,
    /// Number of threads.
    pub thread_count: u32,
    /// Terminal services session id.
    pub session_id: Option<u32>,
}

/// Where a discovered application came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppSource {
    /// `HKLM|HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths`
    AppPaths,
    /// A shortcut under a Start Menu `Programs` folder.
    StartMenu,
    /// An executable found on `PATH`.
    PathExecutable,
    /// An alias in `%LOCALAPPDATA%\Microsoft\WindowsApps`.
    WindowsApps,
}

impl AppSource {
    /// Lower-case wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            AppSource::AppPaths => "app-paths",
            AppSource::StartMenu => "start-menu",
            AppSource::PathExecutable => "path",
            AppSource::WindowsApps => "windows-apps",
        }
    }
}

impl fmt::Display for AppSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An installed or discoverable application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppEntry {
    /// Friendly name shown in the list.
    pub name: String,
    /// Launch target: an executable path, a `.lnk` path or a shell target.
    pub target: String,
    /// Optional command line arguments.
    pub arguments: Option<String>,
    /// Optional working directory.
    pub working_dir: Option<String>,
    /// Where the entry was discovered.
    pub source: AppSource,
}

/// Windows service state (`SERVICE_STATUS.dwCurrentState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceState {
    Stopped,
    StartPending,
    StopPending,
    Running,
    ContinuePending,
    PausePending,
    Paused,
    /// The service reported a state this build does not know about.
    #[default]
    Unknown,
}

impl ServiceState {
    /// Lower-case wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ServiceState::Stopped => "stopped",
            ServiceState::StartPending => "start-pending",
            ServiceState::StopPending => "stop-pending",
            ServiceState::Running => "running",
            ServiceState::ContinuePending => "continue-pending",
            ServiceState::PausePending => "pause-pending",
            ServiceState::Paused => "paused",
            ServiceState::Unknown => "unknown",
        }
    }
}

impl fmt::Display for ServiceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Service start type (`SERVICE_CONFIG.dwStartType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceStartType {
    Boot,
    System,
    Automatic,
    Manual,
    Disabled,
    /// The service reported a start type this build does not know about.
    #[default]
    Unknown,
}

impl ServiceStartType {
    /// Lower-case wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ServiceStartType::Boot => "boot",
            ServiceStartType::System => "system",
            ServiceStartType::Automatic => "auto",
            ServiceStartType::Manual => "manual",
            ServiceStartType::Disabled => "disabled",
            ServiceStartType::Unknown => "unknown",
        }
    }
}

impl fmt::Display for ServiceStartType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A Windows service registered with the service control manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceEntry {
    /// Service key name, for example `wuauserv`.
    pub name: String,
    /// Human readable display name.
    pub display_name: String,
    /// Current state.
    pub state: ServiceState,
    /// Image path from `QueryServiceConfig`.
    pub binary_path: Option<String>,
    /// Start type from `QueryServiceConfig`.
    pub start_type: ServiceStartType,
    /// Account the service runs as.
    pub account: Option<String>,
    /// Process id of the running service, when applicable.
    pub pid: Option<u32>,
}

/// A top-level window owned by some process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowEntry {
    /// Native window handle, widened to `i64` so the model stays platform
    /// neutral and JSON-safe.
    pub hwnd: i64,
    /// Window title as reported by `GetWindowTextW`.
    pub title: String,
    /// Owning process id.
    pub pid: u32,
    /// Image name of the owning process, when it can be resolved.
    pub process_name: Option<String>,
    /// Whether the window is currently visible.
    pub visible: bool,
    /// Registered window class name, when readable.
    pub class_name: Option<String>,
}

/// Anything the engine can return, tagged by variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LocalEntity {
    File(FileEntry),
    Directory(DirectoryEntry),
    Process(ProcessEntry),
    Application(AppEntry),
    Service(ServiceEntry),
    Window(WindowEntry),
}

impl LocalEntity {
    /// Which domain this entity belongs to.
    #[must_use]
    pub const fn entity_type(&self) -> EntityType {
        match self {
            LocalEntity::File(_) => EntityType::File,
            LocalEntity::Directory(_) => EntityType::Directory,
            LocalEntity::Process(_) => EntityType::Process,
            LocalEntity::Application(_) => EntityType::Application,
            LocalEntity::Service(_) => EntityType::Service,
            LocalEntity::Window(_) => EntityType::Window,
        }
    }

    /// Stable identity, used for de-duplication, usage learning and caching.
    ///
    /// Paths and names are lower-cased because Windows file systems and the
    /// service control manager are case-insensitive.
    #[must_use]
    pub fn id(&self) -> String {
        match self {
            LocalEntity::File(entry) => format!("file:{}", entry.path.to_lowercase()),
            LocalEntity::Directory(entry) => format!("dir:{}", entry.path.to_lowercase()),
            LocalEntity::Process(entry) => format!("process:{}", entry.pid),
            LocalEntity::Application(entry) => format!("app:{}", entry.target.to_lowercase()),
            LocalEntity::Service(entry) => format!("service:{}", entry.name.to_lowercase()),
            LocalEntity::Window(entry) => format!("window:{}", entry.hwnd),
        }
    }

    /// The primary string a query is matched against.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            LocalEntity::File(entry) => &entry.name,
            LocalEntity::Directory(entry) => &entry.name,
            LocalEntity::Process(entry) => &entry.name,
            LocalEntity::Application(entry) => &entry.name,
            LocalEntity::Service(entry) => &entry.name,
            LocalEntity::Window(entry) => &entry.title,
        }
    }

    /// Name shown in the result list (falls back to `name`).
    #[must_use]
    pub fn display_name(&self) -> String {
        match self {
            LocalEntity::Service(entry) if !entry.display_name.is_empty() => {
                entry.display_name.clone()
            }
            _ => self.name().to_string(),
        }
    }

    /// Filesystem location, when the entity has one.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        match self {
            LocalEntity::File(entry) => Some(&entry.path),
            LocalEntity::Directory(entry) => Some(&entry.path),
            LocalEntity::Process(entry) => entry.exe_path.as_deref(),
            LocalEntity::Application(entry) => Some(&entry.target),
            LocalEntity::Service(entry) => entry.binary_path.as_deref(),
            LocalEntity::Window(_) => None,
        }
    }

    /// Secondary line rendered under the name in the UI.
    #[must_use]
    pub fn subtitle(&self) -> String {
        match self {
            LocalEntity::File(_) | LocalEntity::Directory(_) => {
                self.path().unwrap_or("").to_string()
            }
            LocalEntity::Process(entry) => {
                let mut parts = vec![format!("PID {}", entry.pid)];
                if entry.memory_bytes > 0 {
                    parts.push(format!("{:.0} MB", entry.memory_bytes as f64 / 1_048_576.0));
                }
                if let Some(path) = entry.exe_path.as_deref() {
                    parts.push(path.to_string());
                }
                parts.join(" · ")
            }
            LocalEntity::Application(entry) => {
                format!("{} · {}", entry.source, entry.target)
            }
            LocalEntity::Service(entry) => {
                let mut parts = vec![entry.state.to_string(), entry.start_type.to_string()];
                if let Some(path) = entry.binary_path.as_deref() {
                    parts.push(path.to_string());
                }
                parts.join(" · ")
            }
            LocalEntity::Window(entry) => match entry.process_name.as_deref() {
                Some(process) if !process.is_empty() => format!("{process} · PID {}", entry.pid),
                _ => format!("PID {}", entry.pid),
            },
        }
    }

    /// Textual metadata exposed to CLI/MCP/UI consumers that only want a flat
    /// key/value view of a result.
    #[must_use]
    pub fn metadata(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert("entity_type".into(), self.entity_type().to_string());
        match self {
            LocalEntity::File(entry) => {
                map.insert("path".into(), entry.path.clone());
                map.insert("size_bytes".into(), entry.size.to_string());
                if let Some(ext) = &entry.extension {
                    map.insert("extension".into(), ext.clone());
                }
                if let Some(modified) = entry.modified {
                    map.insert("modified_unix_ms".into(), modified.to_string());
                }
                if let Some(created) = entry.created {
                    map.insert("created_unix_ms".into(), created.to_string());
                }
                if let Some(drive) = entry.drive {
                    map.insert("drive".into(), drive.to_string());
                }
                if let Some(file_id) = entry.file_id {
                    map.insert("file_id".into(), file_id.to_string());
                }
            }
            LocalEntity::Directory(entry) => {
                map.insert("path".into(), entry.path.clone());
                if let Some(modified) = entry.modified {
                    map.insert("modified_unix_ms".into(), modified.to_string());
                }
                if let Some(drive) = entry.drive {
                    map.insert("drive".into(), drive.to_string());
                }
                if let Some(file_id) = entry.file_id {
                    map.insert("file_id".into(), file_id.to_string());
                }
            }
            LocalEntity::Process(entry) => {
                map.insert("pid".into(), entry.pid.to_string());
                map.insert("memory_bytes".into(), entry.memory_bytes.to_string());
                map.insert("thread_count".into(), entry.thread_count.to_string());
                if let Some(path) = &entry.exe_path {
                    map.insert("exe_path".into(), path.clone());
                }
                if let Some(parent) = entry.parent_pid {
                    map.insert("parent_pid".into(), parent.to_string());
                }
                if let Some(start) = entry.start_time {
                    map.insert("start_unix_ms".into(), start.to_string());
                }
                if let Some(user) = &entry.username {
                    map.insert("username".into(), user.clone());
                }
                if let Some(session) = entry.session_id {
                    map.insert("session_id".into(), session.to_string());
                }
            }
            LocalEntity::Application(entry) => {
                map.insert("target".into(), entry.target.clone());
                map.insert("source".into(), entry.source.to_string());
                if let Some(arguments) = &entry.arguments {
                    map.insert("arguments".into(), arguments.clone());
                }
                if let Some(dir) = &entry.working_dir {
                    map.insert("working_dir".into(), dir.clone());
                }
            }
            LocalEntity::Service(entry) => {
                map.insert("service_name".into(), entry.name.clone());
                map.insert("display_name".into(), entry.display_name.clone());
                map.insert("state".into(), entry.state.to_string());
                map.insert("start_type".into(), entry.start_type.to_string());
                if let Some(path) = &entry.binary_path {
                    map.insert("binary_path".into(), path.clone());
                }
                if let Some(account) = &entry.account {
                    map.insert("account".into(), account.clone());
                }
                if let Some(pid) = entry.pid {
                    map.insert("pid".into(), pid.to_string());
                }
            }
            LocalEntity::Window(entry) => {
                map.insert("hwnd".into(), entry.hwnd.to_string());
                map.insert("pid".into(), entry.pid.to_string());
                map.insert("visible".into(), entry.visible.to_string());
                if let Some(process) = &entry.process_name {
                    map.insert("process_name".into(), process.clone());
                }
                if let Some(class) = &entry.class_name {
                    map.insert("class_name".into(), class.clone());
                }
            }
        }
        map
    }

    /// Filesystem extension, when the entity has one.
    #[must_use]
    pub fn extension(&self) -> Option<&str> {
        match self {
            LocalEntity::File(entry) => entry.extension.as_deref(),
            LocalEntity::Application(entry) => extension_of(&entry.target),
            LocalEntity::Process(entry) => entry.exe_path.as_deref().and_then(extension_of),
            _ => None,
        }
    }

    /// Best-effort last-modified timestamp as Unix milliseconds.
    #[must_use]
    pub fn modified_ms(&self) -> Option<i64> {
        match self {
            LocalEntity::File(entry) => entry.modified,
            LocalEntity::Directory(entry) => entry.modified,
            LocalEntity::Process(entry) => entry.start_time,
            LocalEntity::Service(_) | LocalEntity::Application(_) | LocalEntity::Window(_) => None,
        }
    }
}

/// Extract a lower-cased extension from a path-like string.
#[must_use]
pub fn extension_of(path: &str) -> Option<&str> {
    let file_name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let (stem, ext) = file_name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() || ext.len() > 32 {
        return None;
    }
    Some(ext)
}

/// Extract the drive letter from a fully qualified path such as `C:\Users`.
#[must_use]
pub fn drive_of(path: &str) -> Option<char> {
    let mut chars = path.chars();
    let letter = chars.next()?;
    let colon = chars.next()?;
    if letter.is_ascii_alphabetic() && colon == ':' {
        Some(letter.to_ascii_uppercase())
    } else {
        None
    }
}

/// Extract the final path component, handling both separators and trailing
/// separators.
#[must_use]
pub fn file_name_of(path: &str) -> String {
    let trimmed = path.trim_end_matches(['\\', '/']);
    trimmed
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(trimmed)
        .to_string()
}
