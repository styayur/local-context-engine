//! # `windows-apps`
//!
//! Application discovery across the four places a launchable app can live on
//! Windows:
//!
//! | source            | how it is found                                              |
//! |-------------------|--------------------------------------------------------------|
//! | `App Paths`       | `HKLM`/`HKCU\...\CurrentVersion\App Paths` registry subkeys   |
//! | `Start Menu`      | `.lnk` files under the machine and user `Programs` folders     |
//! | `PATH`            | every `*.exe` in a directory on `PATH`                         |
//! | `WindowsApps`     | aliases in `%LOCALAPPDATA%\Microsoft\WindowsApps`             |
//!
//! Results are deduplicated by launch target, with the more authoritative
//! source winning (App Paths > Start Menu > WindowsApps > PATH).
//!
//! Enumerating `PATH` is the only slow part, so the whole catalogue is cached
//! and rebuilt on demand rather than on every keystroke.
//!
//! MSIX/AppModel packages are not enumerated yet — see the roadmap in
//! `docs/ARCHITECTURE.md` for the planned `AppModel\Repository` reader.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod discovery;
pub mod provider;
mod registry;

pub use discovery::{discover_apps, AppCatalogue};
pub use provider::AppProvider;
