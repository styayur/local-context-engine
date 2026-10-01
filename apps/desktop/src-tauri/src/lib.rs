//! The Tauri shell for Local Context Engine.
//!
//! The shell owns exactly one [`SearchService`] and exposes it through a small
//! set of commands. There is no search logic here: every command forwards to
//! the same core the CLI and the MCP server use, which is what keeps the three
//! front ends from drifting apart.

use std::sync::Arc;

use search_core::{EntityType, LceError, LocalEntity, SearchResponse};
use search_daemon::{IndexStatus, SearchOptions, SearchService, Settings};
use serde::Serialize;
use tauri::{Manager, State};
use windows_files::IndexReport;

mod shortcuts;

/// Shared application state.
pub struct AppState {
    service: Arc<SearchService>,
}

impl AppState {
    fn new() -> Self {
        Self {
            service: Arc::new(SearchService::bootstrap()),
        }
    }
}

/// The error shape the WebView receives.
///
/// It carries a stable code the UI translates and a developer message that only
/// ends up in the log — never the raw `OsError(5)` the project forbids showing.
#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    /// Stable machine code, for example `volume-access-denied`.
    pub code: String,
    /// Developer-facing detail.
    pub message: String,
    /// Short English fallback for the user.
    pub hint: String,
}

impl From<LceError> for CommandError {
    fn from(error: LceError) -> Self {
        tracing::warn!(code = error.code(), %error, "command failed");
        Self {
            code: error.code().to_string(),
            message: error.to_string(),
            hint: error.hint().to_string(),
        }
    }
}

type CommandResult<T> = Result<T, CommandError>;

/// Search, with the type restriction the UI's filter chips imply.
#[tauri::command]
async fn search(
    state: State<'_, AppState>,
    query: String,
    types: Vec<String>,
    limit: usize,
) -> CommandResult<SearchResponse> {
    let service = Arc::clone(&state.service);
    let entity_types: Vec<EntityType> = types
        .iter()
        .filter_map(|name| EntityType::parse(name))
        .collect();
    let options = SearchOptions {
        types: entity_types,
        limit: Some(limit),
        explain: false,
    };

    // Searching touches the process table and the service control manager, so
    // it runs off the UI thread even though a warm query is fast.
    tauri::async_runtime::spawn_blocking(move || service.search(&query, &options))
        .await
        .map_err(|error| CommandError {
            code: "search-failed".into(),
            message: error.to_string(),
            hint: "The search task could not be joined.".into(),
        })
}

/// The current settings.
#[tauri::command]
fn settings(state: State<'_, AppState>) -> Settings {
    state.service.settings()
}

/// Persist new settings and return the clamped result.
#[tauri::command]
fn update_settings(state: State<'_, AppState>, settings: Settings) -> CommandResult<Settings> {
    state.service.update_settings(settings).map_err(Into::into)
}

/// Index health, entry counts and per-provider state.
#[tauri::command]
async fn index_status(state: State<'_, AppState>) -> CommandResult<IndexStatus> {
    let service = Arc::clone(&state.service);
    tauri::async_runtime::spawn_blocking(move || service.index_status())
        .await
        .map_err(|error| CommandError {
            code: "index-status-failed".into(),
            message: error.to_string(),
            hint: "The index status task could not be joined.".into(),
        })
}

/// Rebuild the filesystem index. This can take a while, so it never runs on the
/// UI thread.
#[tauri::command]
async fn rebuild_index(state: State<'_, AppState>) -> CommandResult<IndexReport> {
    let service = Arc::clone(&state.service);
    tauri::async_runtime::spawn_blocking(move || service.rebuild_index())
        .await
        .map_err(|error| CommandError {
            code: "index-rebuild-failed".into(),
            message: error.to_string(),
            hint: "The rebuild task could not be joined.".into(),
        })?
        .map_err(Into::into)
}

/// Apply incremental index updates.
#[tauri::command]
async fn update_index(state: State<'_, AppState>) -> CommandResult<serde_json::Value> {
    let service = Arc::clone(&state.service);
    tauri::async_runtime::spawn_blocking(move || service.update_index())
        .await
        .map_err(|error| CommandError {
            code: "index-update-failed".into(),
            message: error.to_string(),
            hint: "The update task could not be joined.".into(),
        })?
        .map(|report| serde_json::to_value(report).unwrap_or(serde_json::Value::Null))
        .map_err(Into::into)
}

/// Health of every provider.
#[tauri::command]
async fn providers(state: State<'_, AppState>) -> CommandResult<Vec<search_core::ProviderStats>> {
    let service = Arc::clone(&state.service);
    tauri::async_runtime::spawn_blocking(move || service.provider_stats())
        .await
        .map_err(|error| CommandError {
            code: "providers-failed".into(),
            message: error.to_string(),
            hint: "The provider query could not be joined.".into(),
        })
}

/// Open a result with its registered handler.
#[tauri::command]
fn open_result(state: State<'_, AppState>, entity: LocalEntity) -> CommandResult<()> {
    state.service.open(&entity).map_err(Into::into)
}

/// Reveal a result in Explorer.
#[tauri::command]
fn reveal_result(state: State<'_, AppState>, entity: LocalEntity) -> CommandResult<()> {
    state.service.reveal(&entity).map_err(Into::into)
}

/// Launch an application result.
#[tauri::command]
fn launch_result(state: State<'_, AppState>, entity: LocalEntity) -> CommandResult<()> {
    state.service.open(&entity).map_err(Into::into)
}

/// End a process. `confirmed` is false unless the user accepted the dialog.
#[tauri::command]
fn terminate_process(state: State<'_, AppState>, pid: u32, confirmed: bool) -> CommandResult<()> {
    state
        .service
        .terminate_process(pid, confirmed)
        .map_err(Into::into)
}

/// Remember a selection for ranking.
#[tauri::command]
fn record_selection(
    state: State<'_, AppState>,
    query: String,
    entity_id: String,
) -> CommandResult<()> {
    state
        .service
        .record_selection(&query, &entity_id)
        .map_err(Into::into)
}

/// Forget the usage history.
#[tauri::command]
fn reset_usage(state: State<'_, AppState>) -> CommandResult<()> {
    state.service.reset_usage().map_err(Into::into)
}

/// Start the desktop application.
pub fn run() {
    init_tracing();
    shortcuts::install_logging();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            search,
            settings,
            update_settings,
            index_status,
            rebuild_index,
            update_index,
            providers,
            open_result,
            reveal_result,
            launch_result,
            terminate_process,
            record_selection,
            reset_usage,
        ])
        .setup(|app| {
            // The configured global shortcut, or Alt+Space, shows and focuses
            // the window from anywhere. A failure to register is not fatal:
            // another application may already own the combination.
            let hotkey = app.state::<AppState>().service.settings().hotkey;
            if let Err(error) = shortcuts::register(app.handle(), &hotkey) {
                tracing::warn!(hotkey, %error, "global shortcut could not be registered");
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("the Tauri application failed to start");
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("LCE_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
