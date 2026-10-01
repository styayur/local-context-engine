//! # `windows-processes`
//!
//! Live process discovery for Windows.
//!
//! The snapshot is read from `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS)` —
//! a documented, stable Win32 API — and enriched per process with
//! `QueryFullProcessImageNameW`, `GetProcessTimes`, `GetProcessMemoryInfo` and
//! `ProcessIdToSessionId`.
//!
//! ## Why not `NtQuerySystemInformation`?
//!
//! `NtQuerySystemInformation(SystemProcessInformation)` is faster because it
//! returns every process in one call, but it is undocumented and its
//! `SYSTEM_PROCESS_INFORMATION` layout has changed between Windows releases.
//! Getting the layout wrong does not produce a compile error; it produces
//! garbage process ids. This crate takes the reliable documented path and
//! documents the fast path as future work — see `docs/ARCHITECTURE.md`.
//!
//! Nothing here is persisted. A process list is only ever a snapshot of *now*.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod provider;
pub mod snapshot;

pub use provider::ProcessProvider;
pub use snapshot::{current_username, list_processes, list_processes_with, ProcessOptions};
