//! # `windows-files`
//!
//! The filesystem half of Local Context Engine.
//!
//! Two interchangeable index backends sit behind one [`provider::FileProvider`]:
//!
//! ```text
//!                      ┌──────────────────┐
//!   \\.\C: ── MFT ────▶ │  mft::build()    │──┐
//!                      └──────────────────┘  │
//!                                            ├──▶ store::FileStore ──▶ search
//!                      ┌──────────────────┐  │
//!   dirs   ── walk ───▶ │  scan::build()   │──┘
//!                      └──────────────────┘
//! ```
//!
//! | backend   | needs admin | gives sizes | warm start                    |
//! |-----------|-------------|-------------|-------------------------------|
//! | `scan`    | no          | yes         | loads the persisted cache     |
//! | `mft-usn` | yes         | no          | replays the USN change journal |
//!
//! `auto` prefers `mft-usn` when the volume handle can be opened and falls back
//! to `scan`, so the provider is always usable. See [`mft`] for why sizes are
//! missing from the MFT path.
//!
//! ## Memory
//!
//! Entries live in a string arena plus fixed-size records, not in one
//! allocation-heavy struct per file: see [`store`] for the arithmetic.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod mft;
pub mod persist;
pub mod provider;
pub mod scan;
pub mod store;
pub mod volume;

pub use mft::{JournalState, MftOutcome, UpdateReport, UsnRecord};
pub use persist::{cache_root, index_path, PersistedIndex, CACHE_FORMAT_VERSION};
pub use provider::{
    collect_candidates, default_volumes, FileProvider, IndexBackend, CANDIDATE_CAP,
};
pub use scan::{IndexConfig, IndexReport};
pub use store::{FileRecord, FileStore, FLAG_DELETED, FLAG_DIRECTORY, FLAG_ROOT};
pub use volume::{list_volumes, root_for_drive, volume_of_path, VolumeSpec};
