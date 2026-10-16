//! The event-driven index supervisor.
//!
//! ```text
//!                    IndexSupervisor
//!                    ├── VolumeWorker(C:)
//!                    ├── VolumeWorker(D:)
//!                    └── VolumeWorker(...)
//! ```
//!
//! One thread per NTFS volume tails that volume's USN journal. The thread does
//! **not** poll: `FSCTL_READ_USN_JOURNAL` is called with a `Timeout` and a
//! `BytesToWaitFor`, so the kernel holds the call open until the journal
//! actually moves. Between wake-ups the thread is blocked, which is why idle
//! CPU stays at zero.
//!
//! Failure is contained per volume. A worker that cannot read its journal backs
//! off, marks *that* volume degraded, and leaves every other volume — and the
//! search service — running. The supervisor thread only watches the workers;
//! it never touches the index.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use search_core::clock;

use crate::journal::{RebuildReason, VolumeSyncStatus};
use crate::mft::{self, JournalReadOptions};
use crate::provider::FileProvider;
use crate::volume::{VolumeId, VolumeSpec};

/// Lifecycle state of one volume's worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkerState {
    /// Spawned but has not completed a first read.
    Starting,
    /// Tailing the journal.
    Running,
    /// A read failed and the worker is waiting before retrying.
    BackingOff,
    /// Repeated failures; the worker is still alive but not making progress.
    Failed,
    /// The worker was asked to stop, or the process is shutting down.
    Stopped,
}

impl WorkerState {
    /// Lower-case wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            WorkerState::Starting => "starting",
            WorkerState::Running => "running",
            WorkerState::BackingOff => "backing-off",
            WorkerState::Failed => "failed",
            WorkerState::Stopped => "stopped",
        }
    }

    /// Whether the worker is doing its job.
    #[must_use]
    pub const fn is_healthy(self) -> bool {
        matches!(self, WorkerState::Running | WorkerState::Starting)
    }
}

impl std::fmt::Display for WorkerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one volume's worker is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeWorkerStatus {
    /// Mount point label.
    pub volume: String,
    /// Stable volume identity.
    pub volume_id: VolumeId,
    /// Lifecycle state.
    pub state: WorkerState,
    /// Index freshness for this volume.
    pub sync: VolumeSyncStatus,
    /// How many times the worker has restarted its tail.
    pub restarts: u32,
    /// Journal records applied by this worker.
    pub mutations_applied: u64,
    /// Journal reads that produced work.
    pub batches: u64,
    /// Last time the worker made progress.
    pub last_progress_ms: Option<i64>,
    /// Last error, in the user's language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl VolumeWorkerStatus {
    /// Whether this volume is being kept up to date.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.state.is_healthy() && self.sync.is_current()
    }
}

/// What the supervisor as a whole is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisorStatus {
    /// Whether any worker thread is alive.
    pub running: bool,
    /// One entry per volume.
    pub workers: Vec<VolumeWorkerStatus>,
    /// Total mutations applied by every worker.
    pub total_mutations: u64,
    /// Whether every volume is healthy.
    pub all_healthy: bool,
    /// Volumes that are not being kept current.
    pub degraded: Vec<String>,
}

impl SupervisorStatus {
    /// A status describing a supervisor that is not running.
    #[must_use]
    pub fn stopped() -> Self {
        Self {
            running: false,
            workers: Vec::new(),
            total_mutations: 0,
            all_healthy: false,
            degraded: Vec::new(),
        }
    }
}

/// Shared state between the supervisor and its workers.
#[derive(Debug)]
struct Shared {
    provider: Arc<FileProvider>,
    workers: Mutex<BTreeMap<VolumeId, VolumeWorkerStatus>>,
    mutate_events: AtomicU64,
    running: AtomicBool,
}

/// Owns one tailing thread per volume.
#[derive(Debug)]
pub struct IndexSupervisor {
    shared: Arc<Shared>,
    volumes: Vec<VolumeSpec>,
    stop: Arc<AtomicBool>,
    handles: Mutex<Vec<JoinHandle<()>>>,
    options: JournalReadOptions,
    max_backoff: Duration,
}

impl IndexSupervisor {
    /// Build a supervisor for a provider and a set of volumes.
    #[must_use]
    pub fn new(provider: Arc<FileProvider>, volumes: Vec<VolumeSpec>) -> Self {
        Self {
            shared: Arc::new(Shared {
                provider,
                workers: Mutex::new(BTreeMap::new()),
                mutate_events: AtomicU64::new(0),
                running: AtomicBool::new(false),
            }),
            volumes,
            stop: Arc::new(AtomicBool::new(false)),
            handles: Mutex::new(Vec::new()),
            options: JournalReadOptions::tailing(),
            max_backoff: Duration::from_secs(30),
        }
    }

    /// Build a supervisor for a provider's configured volumes.
    #[must_use]
    pub fn for_provider(provider: Arc<FileProvider>) -> Self {
        let volumes = provider.volumes();
        Self::new(provider, volumes)
    }

    /// Start one worker thread per NTFS volume.
    ///
    /// Returns `false` when the supervisor is already running or there is
    /// nothing to tail. Starting twice is a no-op rather than an error, because
    /// the desktop shell may re-enter this on every settings change.
    pub fn start(&self) -> bool {
        if self.shared.running.swap(true, Ordering::SeqCst) {
            return false;
        }
        self.stop.store(false, Ordering::SeqCst);

        let mut handles = Vec::new();
        for volume in self.volumes.iter().filter(|volume| volume.is_ntfs) {
            self.shared
                .workers
                .lock()
                .map(|mut workers| {
                    workers.insert(
                        volume.identity.id.clone(),
                        VolumeWorkerStatus {
                            volume: volume.label(),
                            volume_id: volume.identity.id.clone(),
                            state: WorkerState::Starting,
                            sync: VolumeSyncStatus::Rebuilding,
                            restarts: 0,
                            mutations_applied: 0,
                            batches: 0,
                            last_progress_ms: None,
                            last_error: None,
                        },
                    )
                })
                .ok();

            let shared = Arc::clone(&self.shared);
            let stop = Arc::clone(&self.stop);
            let volume = volume.clone();
            let options = self.options;
            let max_backoff = self.max_backoff;
            handles.push(std::thread::spawn(move || {
                run_worker(&shared, &volume, options, max_backoff, &stop);
            }));
        }

        if handles.is_empty() {
            self.shared.running.store(false, Ordering::SeqCst);
            return false;
        }
        if let Ok(mut guard) = self.handles.lock() {
            *guard = handles;
        }
        true
    }

    /// Ask every worker to stop and wait for them.
    ///
    /// The wait is bounded by the journal read timeout, so shutdown is prompt
    /// rather than dependent on there being no filesystem activity.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let handles = self
            .handles
            .lock()
            .map(|mut guard| std::mem::take(&mut *guard))
            .unwrap_or_default();
        for handle in handles {
            let _ = handle.join();
        }
        self.shared.running.store(false, Ordering::SeqCst);
    }

    /// Whether worker threads are alive.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }

    /// Current status of every worker.
    #[must_use]
    pub fn status(&self) -> SupervisorStatus {
        let workers: Vec<VolumeWorkerStatus> = self
            .shared
            .workers
            .lock()
            .map(|guard| guard.values().cloned().collect())
            .unwrap_or_default();
        let degraded: Vec<String> = workers
            .iter()
            .filter(|worker| !worker.is_healthy())
            .map(|worker| worker.volume.clone())
            .collect();

        SupervisorStatus {
            running: self.is_running(),
            all_healthy: !workers.is_empty() && degraded.is_empty(),
            degraded,
            total_mutations: self.shared.mutate_events.load(Ordering::Relaxed),
            workers,
        }
    }
}

impl Drop for IndexSupervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_worker(
    shared: &Arc<Shared>,
    volume: &VolumeSpec,
    options: JournalReadOptions,
    max_backoff: Duration,
    stop: &AtomicBool,
) {
    let mut backoff = Duration::from_millis(200);
    let mut failures = 0u32;
    let mut restarts = 0u32;

    update_worker(shared, &volume.identity.id, |worker| {
        worker.state = WorkerState::Running;
    });

    while !stop.load(Ordering::Relaxed) {
        match shared.provider.poll_volume(volume, options) {
            Ok(report) => {
                failures = 0;
                backoff = Duration::from_millis(200);

                let applied = report.applied.applied() as u64;
                if applied > 0 {
                    shared.mutate_events.fetch_add(applied, Ordering::Relaxed);
                }
                let status = shared
                    .provider
                    .volume_journal(&volume.identity.id)
                    .map_or(VolumeSyncStatus::Healthy, |state| state.status);

                update_worker(shared, &volume.identity.id, |worker| {
                    worker.state = WorkerState::Running;
                    worker.sync = status;
                    worker.mutations_applied += applied;
                    if applied > 0 {
                        worker.batches += 1;
                        worker.last_progress_ms = Some(clock::now_ms());
                    }
                    worker.last_error = None;
                });

                // Compaction is a write-lock operation, so it is done here —
                // on the worker's thread, between journal reads, never inside
                // one.
                if shared.provider.compact_accelerators_if_needed() {
                    tracing::info!(volume = volume.label(), "compacted search accelerators");
                }
            }
            Err(error) => {
                failures += 1;
                restarts += 1;
                let failing = backoff >= max_backoff;
                update_worker(shared, &volume.identity.id, |worker| {
                    worker.state = if failing {
                        WorkerState::Failed
                    } else {
                        WorkerState::BackingOff
                    };
                    worker.restarts = restarts;
                    worker.sync = if error.code() == "volume-access-denied" {
                        VolumeSyncStatus::PermissionDenied
                    } else {
                        VolumeSyncStatus::Stale
                    };
                    worker.last_error = Some(error.hint().to_string());
                });

                tracing::warn!(
                    volume = volume.label(),
                    code = error.code(),
                    failures,
                    "volume worker backing off"
                );

                // Sleep in small slices so a shutdown request is honoured
                // promptly even while backing off.
                let mut slept = Duration::ZERO;
                while slept < backoff && !stop.load(Ordering::Relaxed) {
                    let slice = Duration::from_millis(100);
                    std::thread::sleep(slice);
                    slept += slice;
                }
                backoff = (backoff * 2).min(max_backoff);
            }
        }
    }

    update_worker(shared, &volume.identity.id, |worker| {
        worker.state = WorkerState::Stopped;
    });
}

fn update_worker(
    shared: &Arc<Shared>,
    volume_id: &VolumeId,
    apply: impl FnOnce(&mut VolumeWorkerStatus),
) {
    if let Ok(mut workers) = shared.workers.lock() {
        if let Some(worker) = workers.get_mut(volume_id) {
            apply(worker);
        }
    }
}

/// Whether this build can tail journals at all (it needs elevation).
#[must_use]
pub fn tailing_is_available(volumes: &[VolumeSpec]) -> bool {
    mft::is_available(volumes)
}

/// Why a volume would need a rebuild, as the supervisor sees it.
#[must_use]
pub const fn rebuild_reason_for(status: VolumeSyncStatus) -> Option<RebuildReason> {
    match status {
        VolumeSyncStatus::Stale => Some(RebuildReason::ReadFailed),
        VolumeSyncStatus::PermissionDenied => Some(RebuildReason::NewVolume),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::IndexBackend;
    use crate::scan::IndexConfig;
    use crate::scan::TempTree;

    fn provider() -> Arc<FileProvider> {
        let tree = TempTree::new("supervisor").unwrap();
        tree.write("seed.txt", b"x").unwrap();
        let config = IndexConfig {
            volumes: vec![VolumeSpec::synthetic(
                tree.root().to_string_lossy().to_string(),
                'C',
                true,
            )],
            ..IndexConfig::default()
        };
        let label = format!(
            "test-sup-{}",
            tree.root()
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default()
        );
        let provider = Arc::new(FileProvider::new(config, IndexBackend::Scan, label));
        provider.rebuild().unwrap();
        // Keep the temp tree alive for the lifetime of the test process.
        std::mem::forget(tree);
        provider
    }

    #[test]
    fn a_supervisor_over_no_volumes_does_not_start() {
        let provider = provider();
        let supervisor = IndexSupervisor::new(provider, Vec::new());
        assert!(!supervisor.start());
        assert!(!supervisor.is_running());
    }

    #[test]
    fn stopping_a_supervisor_that_never_started_is_a_no_op() {
        let provider = provider();
        let supervisor = IndexSupervisor::new(provider, Vec::new());
        supervisor.stop();
        assert!(!supervisor.is_running());
    }

    #[test]
    fn a_scan_index_has_no_journal_to_tail() {
        let provider = provider();
        // The scan backend registers no journals, so the provider reports
        // "nothing to update" rather than pretending it refreshed anything.
        assert!(provider.update().unwrap().is_none());
    }

    #[test]
    fn starting_a_supervisor_over_an_ntfs_volume_spawns_a_worker() {
        let provider = provider();
        let volume = provider.volumes().into_iter().next().unwrap();
        let supervisor = IndexSupervisor::new(provider, vec![volume]);
        assert!(supervisor.start());
        assert!(!supervisor.start(), "starting twice must be a no-op");

        // Give the worker a moment to make its first (failing, unelevated)
        // journal read; the point is that it is alive and tracked.
        std::thread::sleep(Duration::from_millis(150));
        let status = supervisor.status();
        assert_eq!(status.workers.len(), 1);
        assert!(status.running);

        supervisor.stop();
        assert!(!supervisor.is_running());
        assert_eq!(supervisor.status().workers[0].state, WorkerState::Stopped);
    }

    #[test]
    fn a_worker_state_reports_its_own_health() {
        let worker = VolumeWorkerStatus {
            volume: "C:".into(),
            volume_id: VolumeId::from_serial(1),
            state: WorkerState::Running,
            sync: VolumeSyncStatus::Healthy,
            restarts: 0,
            mutations_applied: 0,
            batches: 0,
            last_progress_ms: None,
            last_error: None,
        };
        assert!(worker.is_healthy());

        let degraded = VolumeWorkerStatus {
            sync: VolumeSyncStatus::Stale,
            ..worker.clone()
        };
        assert!(!degraded.is_healthy());
    }

    #[test]
    fn a_stopped_status_is_explicit() {
        let status = SupervisorStatus::stopped();
        assert!(!status.running);
        assert!(!status.all_healthy);
        assert!(status.workers.is_empty());
    }

    #[test]
    fn worker_states_have_stable_names() {
        for state in [
            WorkerState::Starting,
            WorkerState::Running,
            WorkerState::BackingOff,
            WorkerState::Failed,
            WorkerState::Stopped,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            assert!(json.contains(state.as_str()), "{json}");
        }
        assert!(WorkerState::Running.is_healthy());
        assert!(!WorkerState::Failed.is_healthy());
    }

    #[test]
    fn stale_and_permission_denied_map_to_rebuild_reasons() {
        assert!(rebuild_reason_for(VolumeSyncStatus::Stale).is_some());
        assert!(rebuild_reason_for(VolumeSyncStatus::PermissionDenied).is_some());
        assert!(rebuild_reason_for(VolumeSyncStatus::Healthy).is_none());
    }
}
