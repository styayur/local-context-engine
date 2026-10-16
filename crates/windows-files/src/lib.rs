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

pub mod accelerators;
pub mod journal;
pub mod mft;
pub mod mutation;
pub mod persist;
pub mod planner;
pub mod provider;
pub mod scan;
pub mod store;
pub mod volume;
pub mod worker;

pub use accelerators::{
    AcceleratorStats, ExtensionIndex, PrefixEntry, PrefixIndex, RecordId, SearchAccelerators,
    TrigramIndex, MIN_TRIGRAM_QUERY_LEN, PREFIX_KEY_LEN,
};
pub use journal::{
    IdentityReconciliation, JournalRegistry, RebuildReason, SyncDecision, VolumeJournalState,
    VolumeSyncStatus, JOURNAL_SCHEMA_VERSION,
};
pub use mft::{
    apply_journal_batch, mutation_for, read_journal_batches, JournalBatch, JournalReadOptions,
    MftOutcome, QueryUsnJournalData, UpdateReport, UsnRecord, VolumeBatch, VolumeUpdateReport,
};
pub use mutation::{ApplyReport, IndexMutation};
pub use persist::{cache_root, index_path, PersistedIndex, CACHE_FORMAT_VERSION};
pub use planner::{
    linear_plan, plan, CandidateSource, PlanSource, PlannedCandidates, QueryPlanInfo, SearchPlan,
    MAX_PLANNED_CANDIDATES,
};
pub use provider::{
    collect_candidates_linear, collect_candidates_with_plan, default_volumes, explain_plan,
    normalized_key, plan_kind_for, FileProvider, IndexBackend, CANDIDATE_CAP,
};
pub use scan::{IndexConfig, IndexReport};
pub use store::{FileRecord, FileStore, FLAG_DELETED, FLAG_DIRECTORY, FLAG_ROOT};
pub use volume::{
    identity_for_drive, list_volumes, resolve_identity, root_for_drive,
    volume_guid_for_mount_point, volume_of_path, VolumeId, VolumeIdentity, VolumeSpec,
};
pub use worker::{
    tailing_is_available, IndexSupervisor, SupervisorStatus, VolumeWorkerStatus, WorkerState,
};
