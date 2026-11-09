// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, oneshot};
use vigil_types::api::ExitInfo;
use vigil_types::plan::{ServiceConfig, Startup};

use crate::logs::LogStore;
use crate::metrics::MetricsStore;

mod actor;
pub mod notify;
mod policy;
pub mod schedule;
pub mod watch;

pub use policy::{ExitCodeMode, effective_on_failure, effective_on_success, shutdown_code};

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

/// Commands that can be sent to a ServiceActor.
pub enum Cmd {
    Start(oneshot::Sender<anyhow::Result<()>>),
    Stop(oneshot::Sender<anyhow::Result<()>>),
    /// Stop, and reply once the process is gone (after `stop-signal`, at
    /// the latest after the `SIGKILL` at `kill-delay`). For vigild's own
    /// shutdown, which must not end before its services.
    StopWait(oneshot::Sender<()>),
    Restart(oneshot::Sender<anyhow::Result<()>>),
    /// The actor's own view of its state (tests; the overlord builds its
    /// snapshot from events).
    #[cfg(test)]
    Status(oneshot::Sender<Snapshot>),
    /// Forward a signal to the service's process group (if running).
    ForwardSignal(nix::sys::signal::Signal),
    /// Send a signal to the service's process group (`POST /v1/signals`);
    /// the reply says whether there was a process to send it to.
    Signal(nix::sys::signal::Signal, oneshot::Sender<bool>),
    /// The plan changed (`replan`). The new config applies from the next
    /// start; a running process is left alone (the overlord restarts it,
    /// see `overlord/restart.rs`).
    UpdateConfig(Box<ServiceConfig>),
    /// A probe of `check` that started at `started_at` succeeded
    /// (for `ready: check`).
    CheckPassed {
        check: String,
        started_at: std::time::Instant,
    },
    /// Graceful shutdown of the actor (kills child if running).
    Shutdown,
}

/// Point-in-time view of a service returned via `Cmd::Status`.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub name: String,
    pub state: crate::state::ServiceState,
    pub since: DateTime<Utc>,
    pub startup: Startup,
    /// How the last process ended (raw, before `exit-code-map`).
    pub last_exit: Option<ExitInfo>,
    /// Last `STATUS=` text received via sd_notify.
    pub status_message: Option<String>,
    /// Next scheduled run (`schedule`).
    pub next_run: Option<DateTime<Utc>>,
}

impl Snapshot {
    /// A service that has not run yet: `inactive` since now.
    pub fn inactive(name: String, startup: Startup) -> Self {
        Self {
            name,
            state: crate::state::ServiceState::Inactive,
            since: Utc::now(),
            startup,
            last_exit: None,
            status_message: None,
            next_run: None,
        }
    }
}

/// Event emitted by the actor to the Overlord.
#[derive(Debug)]
pub struct Event {
    pub service: String,
    pub kind: EventKind,
}

#[derive(Debug)]
pub enum EventKind {
    StateChanged {
        new_state: crate::state::ServiceState,
    },
    ProcessExited {
        #[allow(dead_code)]
        success: bool,
        /// `None` if waiting for the process failed.
        exit: Option<ExitInfo>,
    },
    /// A service policy requires the daemon to exit with the given code.
    DaemonShutdown { exit_code: i32 },
    /// The service sent a `STATUS=` text (or a new start cleared it).
    StatusMessage { message: Option<String> },
    /// The next scheduled run changed (`None`: no schedule armed).
    NextRun { at: Option<DateTime<Utc>> },
    /// The service failed: `event` is `backoff`, `error` or `failed`
    /// (alert event names), `message` the detail.
    Unhealthy {
        event: &'static str,
        message: String,
    },
    /// The service is healthy again: `active` for `healthy-time`, or a
    /// oneshot run succeeded.
    Healthy,
}

/// What every service actor gets from the daemon. The same for all
/// services; cloned into each actor.
#[derive(Clone)]
pub struct Context {
    /// Events to the Overlord.
    pub event_tx: mpsc::Sender<Event>,
    pub log_store: Arc<LogStore>,
    pub metrics: Arc<MetricsStore>,
    /// How vigild's exit code is chosen on a shutdown policy.
    pub exit_code_mode: ExitCodeMode,
    /// `<path-prefix>/notify`, where `ready: notify` sockets are created.
    pub notify_dir: std::path::PathBuf,
}

/// Handle returned to the caller; wraps the command sender.
pub struct Handle {
    pub tx: mpsc::Sender<Cmd>,
}

/// Spawn a service actor task and return its handle.
pub fn spawn(name: String, config: ServiceConfig, ctx: Context) -> Handle {
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(actor::run(name, config, rx, ctx));
    Handle { tx }
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

pub(super) const DEFAULT_KILL_DELAY: Duration = Duration::from_secs(5);
pub(crate) const DEFAULT_BACKOFF_DELAY: Duration = Duration::from_millis(500);
pub(super) const DEFAULT_BACKOFF_FACTOR: f64 = 2.0;
pub(crate) const DEFAULT_BACKOFF_LIMIT: Duration = Duration::from_secs(30);
pub(super) const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(30);
/// Default `healthy-time`.
pub const DEFAULT_HEALTHY_TIME: Duration = Duration::from_secs(10);
/// Bounds of `healthy-time`.
pub const MIN_HEALTHY_TIME: Duration = Duration::from_secs(1);
pub const MAX_HEALTHY_TIME: Duration = Duration::from_secs(3600);
