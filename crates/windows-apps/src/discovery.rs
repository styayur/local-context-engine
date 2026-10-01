//! Application catalogue discovery.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use search_core::{file_name_of, AppEntry, AppSource};

use crate::registry::{self, RegRoot};

/// The full set of discovered applications.
#[derive(Debug, Clone, Default)]
pub struct AppCatalogue {
    entries: Vec<AppEntry>,
}

impl AppCatalogue {
    /// Build the catalogue from every supported source.
    ///
    /// Never fails: a source that cannot be read (a registry key that needs
    /// elevation, an unreadable Start Menu) simply contributes nothing.
    #[must_use]
    pub fn build() -> Self {
        let mut entries: Vec<AppEntry> = Vec::new();
        entries.extend(from_app_paths());
        entries.extend(from_start_menu());
        entries.extend(from_windows_apps());
        entries.extend(from_path());

        let deduped = deduplicate(entries);
        Self { entries: deduped }
    }

    /// The catalogue entries.
    #[must_use]
    pub fn entries(&self) -> &[AppEntry] {
        &self.entries
    }

    /// How many applications were discovered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing was discovered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Count of entries per source, for the index panel.
    #[must_use]
    pub fn source_counts(&self) -> HashMap<AppSource, usize> {
        let mut counts = HashMap::new();
        for entry in &self.entries {
            *counts.entry(entry.source).or_insert(0) += 1;
        }
        counts
    }
}

/// Discover every application on this machine.
#[must_use]
pub fn discover_apps() -> Vec<AppEntry> {
    AppCatalogue::build().entries
}

/// Rank a source so that deduplication keeps the best one.
const fn source_rank(source: AppSource) -> u8 {
    match source {
        AppSource::AppPaths => 0,
        AppSource::StartMenu => 1,
        AppSource::WindowsApps => 2,
        AppSource::PathExecutable => 3,
    }
}

fn deduplicate(entries: Vec<AppEntry>) -> Vec<AppEntry> {
    let mut best: HashMap<String, AppEntry> = HashMap::new();
    for entry in entries {
        if entry.target.trim().is_empty() {
            continue;
        }
        let key = entry.target.to_lowercase();
        match best.get(&key) {
            Some(existing) if source_rank(existing.source) <= source_rank(entry.source) => {}
            _ => {
                best.insert(key, entry);
            }
        }
    }
    let mut result: Vec<AppEntry> = best.into_values().collect();
    result.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.target.cmp(&right.target))
    });
    result
}

fn from_app_paths() -> Vec<AppEntry> {
    let mut entries = Vec::new();
    for (root, flags) in [
        (RegRoot::LocalMachine, registry::KEY_WOW64_64KEY),
        (RegRoot::LocalMachine, registry::KEY_WOW64_32KEY),
        (RegRoot::CurrentUser, 0),
    ] {
        let Some(key) = registry::open(
            root,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths",
            registry::KEY_READ | flags,
        ) else {
            continue;
        };
        for subkey_name in registry::enum_subkeys(key) {
            let Some(subkey) = registry::open(
                root,
                &format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{subkey_name}"),
                registry::KEY_READ | flags,
            ) else {
                continue;
            };
            let target = registry::query_string(subkey, None).unwrap_or_default();
            let working_dir =
                registry::query_string(subkey, Some("Path")).filter(|dir| !dir.is_empty());
            registry::close(subkey);
            if target.trim().is_empty() {
                continue;
            }
            let display = prettify(&subkey_name);
            entries.push(AppEntry {
                name: display,
                target,
                arguments: None,
                working_dir,
                source: AppSource::AppPaths,
            });
        }
        registry::close(key);
    }
    entries
}

fn from_start_menu() -> Vec<AppEntry> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(program_data) = std::env::var("ProgramData") {
        roots.push(Path::new(&program_data).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Ok(app_data) = std::env::var("APPDATA") {
        roots.push(Path::new(&app_data).join(r"Microsoft\Windows\Start Menu\Programs"));
    }

    let mut entries = Vec::new();
    for root in roots {
        walk_shortcuts(&root, 0, &mut entries);
    }
    entries
}

fn walk_shortcuts(directory: &Path, depth: usize, out: &mut Vec<AppEntry>) {
    const MAX_DEPTH: usize = 5;
    if depth > MAX_DEPTH || out.len() > 5_000 {
        return;
    }
    let Ok(reader) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk_shortcuts(&path, depth + 1, out);
            continue;
        }
        let is_shortcut = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"));
        if !is_shortcut {
            continue;
        }
        let Some(stem) = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
        else {
            continue;
        };
        if stem.trim().is_empty() {
            continue;
        }
        out.push(AppEntry {
            name: prettify(&stem),
            target: path.to_string_lossy().to_string(),
            arguments: None,
            working_dir: path
                .parent()
                .map(|parent| parent.to_string_lossy().to_string()),
            source: AppSource::StartMenu,
        });
    }
}

fn from_windows_apps() -> Vec<AppEntry> {
    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
        return Vec::new();
    };
    let directory = PathBuf::from(local_app_data).join("Microsoft\\WindowsApps");
    executables_in(&directory, AppSource::WindowsApps)
}

fn from_path() -> Vec<AppEntry> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for directory in std::env::split_paths(&path) {
        if !directory.is_dir() {
            continue;
        }
        entries.extend(executables_in(&directory, AppSource::PathExecutable));
        if entries.len() > 20_000 {
            break;
        }
    }
    entries
}

fn executables_in(directory: &Path, source: AppSource) -> Vec<AppEntry> {
    let Ok(reader) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for entry in reader.flatten() {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            continue;
        }
        let Some(stem) = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
        else {
            continue;
        };
        if stem.trim().is_empty() {
            continue;
        }
        entries.push(AppEntry {
            name: prettify(&stem),
            target: path.to_string_lossy().to_string(),
            arguments: None,
            working_dir: Some(directory.to_string_lossy().to_string()),
            source,
        });
    }
    entries
}

/// Turn `visual-studio-code` or `Code.exe` into `Visual Studio Code`.
#[must_use]
pub fn prettify(raw: &str) -> String {
    let stem = match raw.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && extension.eq_ignore_ascii_case("exe") => {
            stem
        }
        _ => raw,
    };

    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    for c in stem.chars() {
        if c.is_whitespace() || c == '-' || c == '_' || c == '.' {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(c);
    }
    if !current.is_empty() {
        words.push(current);
    }
    if words.is_empty() {
        return stem.to_string();
    }

    words
        .into_iter()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) if first.is_lowercase() => {
                    format!("{}{}", first.to_uppercase(), chars.as_str())
                }
                _ => word,
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The display name for a launched target, used when a source has no name.
#[must_use]
pub fn display_name_for_target(target: &str) -> String {
    prettify(&file_name_of(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prettify_splits_separators_and_capitalises() {
        assert_eq!(prettify("visual-studio-code"), "Visual Studio Code");
        assert_eq!(prettify("Code.exe"), "Code");
        assert_eq!(prettify("notepad++"), "Notepad++");
        assert_eq!(prettify("git_bash"), "Git Bash");
    }

    #[test]
    fn prettify_keeps_unicode_names_intact() {
        assert_eq!(prettify("微信"), "微信");
    }

    #[test]
    fn deduplication_prefers_app_paths_over_path_entries() {
        let entries = vec![
            AppEntry {
                name: "Code".into(),
                target: r"C:\Code.exe".into(),
                arguments: None,
                working_dir: None,
                source: AppSource::PathExecutable,
            },
            AppEntry {
                name: "Visual Studio Code".into(),
                target: r"C:\Code.exe".into(),
                arguments: None,
                working_dir: None,
                source: AppSource::AppPaths,
            },
        ];
        let deduped = deduplicate(entries);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].source, AppSource::AppPaths);
        assert_eq!(deduped[0].name, "Visual Studio Code");
    }

    #[test]
    fn deduplication_is_case_insensitive_on_the_target() {
        let entries = vec![
            AppEntry {
                name: "A".into(),
                target: r"C:\App.exe".into(),
                arguments: None,
                working_dir: None,
                source: AppSource::StartMenu,
            },
            AppEntry {
                name: "B".into(),
                target: r"c:\APP.EXE".into(),
                arguments: None,
                working_dir: None,
                source: AppSource::PathExecutable,
            },
        ];
        assert_eq!(deduplicate(entries).len(), 1);
    }

    #[test]
    fn entries_without_a_target_are_dropped() {
        let entries = vec![AppEntry {
            name: "Broken".into(),
            target: "   ".into(),
            arguments: None,
            working_dir: None,
            source: AppSource::StartMenu,
        }];
        assert!(deduplicate(entries).is_empty());
    }

    #[test]
    fn a_real_catalogue_can_be_built() {
        let catalogue = AppCatalogue::build();
        // A Windows machine always has at least one discoverable executable on
        // PATH, even in a minimal container.
        assert!(!catalogue.is_empty(), "expected at least one application");
        assert!(catalogue
            .entries()
            .iter()
            .all(|entry| !entry.target.is_empty()));
    }

    #[test]
    fn catalogue_is_sorted_case_insensitively() {
        let catalogue = AppCatalogue::build();
        let names: Vec<String> = catalogue
            .entries()
            .iter()
            .map(|entry| entry.name.to_lowercase())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }
}
