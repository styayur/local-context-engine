//! Persisted user settings.
//!
//! Settings live in `%LOCALAPPDATA%\LocalContextEngine\settings.json`. Nothing
//! here is ever sent anywhere: the file exists so that your preferences survive
//! a restart, not so that they can be synced.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use windows_files::{IndexBackend, IndexConfig};

use search_core::{LceError, MAX_RESULT_LIMIT};
/// Re-exported so the CLI and UI can find the cache directory through the same
/// public surface as the rest of the settings.
pub use windows_files::cache_root;

/// Which language the interface uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    /// Follow the Windows user locale on first launch.
    #[default]
    System,
    /// Simplified Chinese.
    ZhCn,
    /// English.
    EnUs,
}

impl Language {
    /// Lower-case wire name, also the i18n bundle key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Language::System => "system",
            Language::ZhCn => "zh-CN",
            Language::EnUs => "en-US",
        }
    }

    /// Resolve to a concrete locale, consulting the OS for `System`.
    #[must_use]
    pub fn resolve(self) -> &'static str {
        match self {
            Language::ZhCn => "zh-CN",
            Language::EnUs => "en-US",
            Language::System => system_locale(),
        }
    }

    /// Parse a `--lang` value.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim().to_ascii_lowercase().as_str() {
            "system" | "auto" => Language::System,
            "zh" | "zh-cn" | "zh_cn" | "chinese" => Language::ZhCn,
            "en" | "en-us" | "en_us" | "english" => Language::EnUs,
            _ => return None,
        })
    }
}

/// Colour scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the Windows app theme.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

/// Everything the user can change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Interface language.
    pub language: Language,
    /// Colour scheme.
    pub theme: Theme,
    /// Maximum number of results returned for one query.
    pub result_limit: usize,
    /// Whether the fuzzy fallback participates in scoring.
    pub fuzzy_matching: bool,
    /// Whether previously selected results are boosted.
    pub usage_ranking: bool,
    /// Which index backend to prefer.
    pub index_backend: IndexBackend,
    /// Drive letters to index. Empty means "every fixed local volume".
    pub indexed_drives: Vec<char>,
    /// Hard ceiling on indexed entries.
    pub max_index_entries: usize,
    /// Whether directory entries are indexed.
    pub include_directories: bool,
    /// Global shortcut, stored as an Electron-style accelerator string.
    pub hotkey: String,
    /// Whether hidden windows are included in window results.
    pub include_hidden_windows: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::System,
            theme: Theme::System,
            result_limit: search_core::DEFAULT_RESULT_LIMIT,
            fuzzy_matching: true,
            usage_ranking: true,
            index_backend: IndexBackend::Auto,
            indexed_drives: Vec::new(),
            max_index_entries: 1_000_000,
            include_directories: true,
            hotkey: "Alt+Space".into(),
            include_hidden_windows: false,
        }
    }
}

impl Settings {
    /// Load settings from disk, falling back to defaults.
    #[must_use]
    pub fn load() -> Self {
        Self::load_from(&settings_path())
    }

    /// Load settings from an explicit path.
    #[must_use]
    pub fn load_from(path: &Path) -> Self {
        let Ok(bytes) = std::fs::read(path) else {
            return Self::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            tracing::warn!(path = %path.display(), %error, "settings could not be parsed; using defaults");
            Self::default()
        })
    }

    /// Save settings to disk.
    pub fn save(&self) -> Result<(), LceError> {
        self.save_to(&settings_path())
    }

    /// Save settings to an explicit path.
    pub fn save_to(&self, path: &Path) -> Result<(), LceError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| LceError::io("creating the settings directory", &error))?;
        }
        let encoded = serde_json::to_vec_pretty(self).map_err(|error| LceError::Io {
            action: "encoding settings".into(),
            detail: error.to_string(),
        })?;
        std::fs::write(path, encoded).map_err(|error| LceError::io("writing settings", &error))
    }

    /// Clamp anything that could be set to a nonsense value by hand.
    pub fn normalise(&mut self) {
        self.result_limit = self.result_limit.clamp(1, MAX_RESULT_LIMIT);
        self.max_index_entries = self.max_index_entries.clamp(1_000, 20_000_000);
        self.indexed_drives = self
            .indexed_drives
            .iter()
            .map(|drive| drive.to_ascii_uppercase())
            .filter(char::is_ascii_alphabetic)
            .collect();
        self.indexed_drives.sort_unstable();
        self.indexed_drives.dedup();
    }

    /// Build the filesystem index configuration these settings imply.
    #[must_use]
    pub fn index_config(&self) -> IndexConfig {
        let mut volumes = windows_files::list_volumes();
        if !self.indexed_drives.is_empty() {
            volumes.retain(|volume| self.indexed_drives.contains(&volume.drive));
        }
        IndexConfig {
            volumes,
            max_entries: self.max_index_entries,
            include_directories: self.include_directories,
            ..IndexConfig::default()
        }
    }

    /// The keyboard shortcut, or the documented default.
    #[must_use]
    pub fn hotkey_or_default(&self) -> &str {
        if self.hotkey.trim().is_empty() {
            "Alt+Space"
        } else {
            self.hotkey.as_str()
        }
    }
}

/// Where settings live.
#[must_use]
pub fn settings_path() -> PathBuf {
    cache_root().join("settings.json")
}

/// Where the usage history lives.
#[must_use]
pub fn usage_path() -> PathBuf {
    cache_root().join("usage.json")
}

/// The locale Windows reports for the current user.
///
/// The desktop UI uses the WebView's locale instead; this is the fallback for
/// the CLI and for the first launch of the desktop app.
#[must_use]
pub fn system_locale() -> &'static str {
    // The `LANG` override is honoured first so tests and headless runs are
    // deterministic.
    if let Ok(lang) = std::env::var("LCE_LANG") {
        if lang.to_ascii_lowercase().starts_with("zh") {
            return "zh-CN";
        }
        return "en-US";
    }
    if let Ok(lang) = std::env::var("LANG") {
        if lang.to_ascii_lowercase().starts_with("zh") {
            return "zh-CN";
        }
    }
    "en-US"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_documented_values() {
        let settings = Settings::default();
        assert_eq!(settings.language, Language::System);
        assert_eq!(settings.theme, Theme::System);
        assert_eq!(settings.result_limit, 50);
        assert!(settings.fuzzy_matching);
        assert!(settings.usage_ranking);
        assert_eq!(settings.hotkey_or_default(), "Alt+Space");
    }

    #[test]
    fn normalisation_clamps_and_tidies() {
        let mut settings = Settings {
            result_limit: 0,
            max_index_entries: 10,
            indexed_drives: vec!['d', 'C', 'c', '1'],
            ..Settings::default()
        };
        settings.normalise();
        assert_eq!(settings.result_limit, 1);
        assert_eq!(settings.max_index_entries, 1_000);
        assert_eq!(settings.indexed_drives, vec!['C', 'D']);
    }

    #[test]
    fn language_aliases_parse() {
        assert_eq!(Language::parse("zh-CN"), Some(Language::ZhCn));
        assert_eq!(Language::parse("english"), Some(Language::EnUs));
        assert_eq!(Language::parse("system"), Some(Language::System));
        assert_eq!(Language::parse("klingon"), None);
    }

    #[test]
    fn language_resolution_honours_the_override() {
        std::env::set_var("LCE_LANG", "zh-CN");
        assert_eq!(Language::System.resolve(), "zh-CN");
        std::env::set_var("LCE_LANG", "en-US");
        assert_eq!(Language::System.resolve(), "en-US");
        std::env::remove_var("LCE_LANG");
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = Settings {
            language: Language::ZhCn,
            theme: Theme::Dark,
            result_limit: 80,
            indexed_drives: vec!['C', 'D'],
            index_backend: "scan".parse().unwrap_or_default(),
            ..Settings::default()
        };
        let encoded = serde_json::to_vec(&settings).unwrap();
        let decoded: Settings = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(settings, decoded);
    }

    #[test]
    fn a_missing_settings_file_yields_defaults() {
        let path = cache_root().join("definitely-not-here.json");
        assert_eq!(Settings::load_from(&path), Settings::default());
    }

    #[test]
    fn a_corrupt_settings_file_yields_defaults() {
        let path = cache_root().join(format!("broken-{}.json", std::process::id()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn settings_save_and_reload() {
        let path = cache_root().join(format!("settings-test-{}.json", std::process::id()));
        let settings = Settings {
            result_limit: 77,
            ..Settings::default()
        };
        settings.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path).result_limit, 77);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn the_index_config_follows_the_selected_drives() {
        let settings = Settings {
            indexed_drives: vec!['C'],
            ..Settings::default()
        };
        let config = settings.index_config();
        assert!(config.volumes.iter().all(|volume| volume.drive == 'C'));
    }

    #[test]
    fn the_index_config_uses_every_volume_when_none_are_selected() {
        let settings = Settings::default();
        let config = settings.index_config();
        assert_eq!(config.volumes.len(), windows_files::list_volumes().len());
    }
}
