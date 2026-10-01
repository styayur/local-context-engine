//! # `windows-services`
//!
//! Read-only Windows service discovery.
//!
//! `EnumServicesStatusEx` with `SC_ENUM_PROCESS_INFO` gives name, display name,
//! state and owning process id in a single call; `QueryServiceConfig` adds the
//! binary path, start type and account for each service.
//!
//! This crate never starts, stops or reconfigures a service. The MVP is
//! deliberately read-only here: a search tool that can silently stop
//! `wuauserv` is a search tool nobody should install.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod provider;
pub mod snapshot;

pub use provider::ServiceProvider;
pub use snapshot::{list_services, list_services_with, ServiceQueryHint};
