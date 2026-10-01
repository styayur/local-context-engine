//! # `windows-windows`
//!
//! Top-level window discovery via `EnumWindows`.
//!
//! A window list is a snapshot of *now*: windows appear and disappear
//! constantly, so nothing here is ever persisted. The provider memoises one
//! enumeration for a few hundred milliseconds to coalesce keystrokes.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod provider;
pub mod snapshot;

pub use provider::WindowProvider;
pub use snapshot::list_windows;
