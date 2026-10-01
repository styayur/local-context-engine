//! Global shortcut registration.
//!
//! The developer-tool convention is a single key that summons the palette from
//! anywhere. `Alt+Space` is the default because it is free on a stock Windows
//! desktop; the value is user-configurable in Settings → Keyboard.

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// A parsed shortcut, or a message explaining why it could not be parsed.
pub fn parse(accelerator: &str) -> Result<Shortcut, String> {
    accelerator
        .parse::<Shortcut>()
        .map_err(|error| format!("`{accelerator}` is not a valid shortcut: {error}"))
}

/// Register `accelerator` so it shows and focuses the main window.
pub fn register(app: &AppHandle, accelerator: &str) -> Result<(), String> {
    let hotkey = accelerator.trim();
    let hotkey = if hotkey.is_empty() {
        "Alt+Space"
    } else {
        hotkey
    };
    let shortcut = parse(hotkey)?;

    let handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_app, _shortcut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            if let Some(window) = handle.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        })
        .map_err(|error| error.to_string())
}

/// Log the default shortcut so a failed registration is easy to diagnose.
pub fn install_logging() {
    tracing::debug!(default = "Alt+Space", "global shortcut support enabled");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_accelerator_parses() {
        assert!(parse("Alt+Space").is_ok());
    }

    #[test]
    fn other_accelerators_parse() {
        assert!(parse("Ctrl+Shift+Space").is_ok());
        assert!(parse("Ctrl+Alt+L").is_ok());
    }

    #[test]
    fn nonsense_is_rejected() {
        assert!(parse("Definitely+Not+A+Key").is_err());
        assert!(parse("").is_err());
    }
}
