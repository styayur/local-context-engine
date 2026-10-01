//! Persistence for the usage index.
//!
//! The file is small, plain JSON and entirely optional: delete it and the
//! engine simply stops boosting your frequently chosen results.

use std::path::Path;

use ranking::UsageIndex;
use search_core::LceError;

use crate::config::usage_path;

/// Load the usage index, or an empty one.
#[must_use]
pub fn load() -> UsageIndex {
    load_from(&usage_path())
}

/// Load the usage index from an explicit path.
#[must_use]
pub fn load_from(path: &Path) -> UsageIndex {
    let Ok(bytes) = std::fs::read(path) else {
        return UsageIndex::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        tracing::warn!(path = %path.display(), %error, "usage history could not be parsed");
        UsageIndex::default()
    })
}

/// Save the usage index.
pub fn save(index: &UsageIndex) -> Result<(), LceError> {
    save_to(&usage_path(), index)
}

/// Save the usage index to an explicit path.
pub fn save_to(path: &Path, index: &UsageIndex) -> Result<(), LceError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| LceError::io("creating the usage directory", &error))?;
    }
    let encoded = serde_json::to_vec(index).map_err(|error| LceError::Io {
        action: "encoding usage history".into(),
        detail: error.to_string(),
    })?;
    std::fs::write(path, encoded).map_err(|error| LceError::io("writing usage history", &error))
}

/// Forget everything.
pub fn reset() -> Result<(), LceError> {
    let path = usage_path();
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|error| LceError::io("removing the usage history", &error))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_yields_an_empty_index() {
        let path = crate::config::cache_root().join("no-usage-here.json");
        assert!(load_from(&path).is_empty());
    }

    #[test]
    fn usage_round_trips() {
        let path =
            crate::config::cache_root().join(format!("usage-test-{}.json", std::process::id()));
        let mut index = UsageIndex::default();
        index.record("chrome", "app:c:\\chrome.exe");
        index.record("chrome", "app:c:\\chrome.exe");
        save_to(&path, &index).unwrap();

        let reloaded = load_from(&path);
        assert_eq!(reloaded.count("app:c:\\chrome.exe"), 2);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_corrupt_file_yields_an_empty_index() {
        let path =
            crate::config::cache_root().join(format!("usage-broken-{}.json", std::process::id()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not json at all").unwrap();
        assert!(load_from(&path).is_empty());
        std::fs::remove_file(&path).unwrap();
    }
}
