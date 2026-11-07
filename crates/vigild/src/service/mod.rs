// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, oneshot};
use vigil_types::api::ExitInfo;
use vigil_types::plan::{OnExit, ServiceConfig, Startup};

use crate::logs::LogStore;
use crate::metrics::MetricsStore;

mod actor;
pub mod notify;
pub mod schedule;
pub mod watch;

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

/// Commands that can be sent to a ServiceActor.
pub enum Cmd {
    Start(oneshot::Sender<anyhow::Result<()>>),
    Stop(oneshot::Sender<anyhow::Result<()>>),
    Restart(oneshot::Sender<anyhow::Result<()>>),
    /// The actor's own view of its state (tests; the overlord builds its
    /// snapshot from events).
    #[cfg(test)]
    Status(oneshot::Sender<Snapshot>),
    /// Forward a signal to the service's process group (if running).
    ForwardSignal(nix::sys::signal::Signal),
    /// The plan changed (`replan`). The new config applies from the next
    /// start; a running process is left alone.
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

/// How vigild's own exit code is chosen when a policy shuts it down
/// (`--exit-code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExitCodeMode {
    /// v3 behaviour: 0 for success, 10 for failure.
    Fixed,
    /// The service's (mapped) exit code; `128 + signal` for a signal.
    #[default]
    Propagate,
}

/// vigild's exit code for a shutdown `policy`.
///
/// `success` is the service's classification (`success-exit-codes`), `code`
/// its mapped shell code (`None` if unknown, e.g. for check failures).
/// Returns `None` for policies that do not shut vigild down.
pub fn shutdown_code(
    mode: ExitCodeMode,
    policy: OnExit,
    success: bool,
    code: Option<i32>,
) -> Option<i32> {
    let fixed = if success { 0 } else { 10 };
    match policy {
        OnExit::Restart | OnExit::Ignore => None,
        OnExit::SuccessShutdown => Some(0),
        OnExit::Shutdown => Some(match mode {
            ExitCodeMode::Fixed => fixed,
            ExitCodeMode::Propagate => code.unwrap_or(fixed),
        }),
        // A failure exit must never be 0.
        OnExit::FailureShutdown => Some(match (mode, code) {
            (ExitCodeMode::Propagate, Some(c)) if c != 0 => c,
            _ => 10,
        }),
    }
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

/// Effective `on-success` policy. `None` for a oneshot without one: it ends
/// in state `done`.
pub fn effective_on_success(cfg: &ServiceConfig) -> Option<OnExit> {
    match (cfg.on_success, cfg.service_type.unwrap_or_default()) {
        (Some(p), _) => Some(p),
        (None, vigil_types::plan::ServiceType::Oneshot) => None,
        (None, vigil_types::plan::ServiceType::Simple) => Some(OnExit::Restart),
    }
}

/// Effective `on-failure` policy: `ignore` for scheduled jobs (the next
/// run comes at the next scheduled time), else `restart`.
pub fn effective_on_failure(cfg: &ServiceConfig) -> OnExit {
    cfg.on_failure.unwrap_or(if cfg.schedule.is_some() {
        OnExit::Ignore
    } else {
        OnExit::Restart
    })
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

#[cfg(test)]
mod shutdown_code_tests {
    use super::*;
    use ExitCodeMode::*;
    use OnExit::*;

    #[test]
    fn fixed_mode_keeps_v3_codes() {
        assert_eq!(shutdown_code(Fixed, Shutdown, true, Some(0)), Some(0));
        assert_eq!(shutdown_code(Fixed, Shutdown, false, Some(3)), Some(10));
        assert_eq!(
            shutdown_code(Fixed, FailureShutdown, true, Some(0)),
            Some(10)
        );
        assert_eq!(
            shutdown_code(Fixed, FailureShutdown, false, Some(3)),
            Some(10)
        );
        assert_eq!(
            shutdown_code(Fixed, SuccessShutdown, false, Some(3)),
            Some(0)
        );
    }

    #[test]
    fn propagate_mode_uses_service_code() {
        assert_eq!(shutdown_code(Propagate, Shutdown, false, Some(3)), Some(3));
        assert_eq!(shutdown_code(Propagate, Shutdown, true, Some(0)), Some(0));
        assert_eq!(
            shutdown_code(Propagate, Shutdown, false, Some(143)),
            Some(143)
        );
        assert_eq!(
            shutdown_code(Propagate, FailureShutdown, false, Some(3)),
            Some(3)
        );
        assert_eq!(
            shutdown_code(Propagate, FailureShutdown, true, Some(0)),
            Some(10),
            "failure-shutdown must not exit 0"
        );
        assert_eq!(
            shutdown_code(Propagate, SuccessShutdown, false, Some(3)),
            Some(0)
        );
    }

    #[test]
    fn unknown_code_falls_back_to_fixed() {
        assert_eq!(shutdown_code(Propagate, Shutdown, false, None), Some(10));
        assert_eq!(
            shutdown_code(Propagate, FailureShutdown, false, None),
            Some(10)
        );
    }

    #[test]
    fn non_shutdown_policies_return_none() {
        assert_eq!(shutdown_code(Propagate, Restart, false, Some(1)), None);
        assert_eq!(shutdown_code(Fixed, Ignore, true, Some(0)), None);
    }
}
