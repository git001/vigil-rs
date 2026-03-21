// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, oneshot};
use vigil_types::plan::{ServiceConfig, Startup};

use crate::logs::LogStore;
use crate::metrics::MetricsStore;

mod actor;

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

/// Commands that can be sent to a ServiceActor.
pub enum Cmd {
    Start(oneshot::Sender<anyhow::Result<()>>),
    Stop(oneshot::Sender<anyhow::Result<()>>),
    Restart(oneshot::Sender<anyhow::Result<()>>),
    #[allow(dead_code)] // available for direct status queries
    Status(oneshot::Sender<Snapshot>),
    /// Forward a signal to the service's process group (if running).
    ForwardSignal(nix::sys::signal::Signal),
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
    #[allow(dead_code)] // exposed in API responses (future)
    pub pid: Option<u32>,
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
    },
    /// A service policy requires the daemon to exit with the given code.
    DaemonShutdown { exit_code: i32 },
}

/// Handle returned to the caller; wraps the command sender.
pub struct Handle {
    pub tx: mpsc::Sender<Cmd>,
}

/// Spawn a service actor task and return its handle.
pub fn spawn(
    name: String,
    config: ServiceConfig,
    event_tx: mpsc::Sender<Event>,
    log_store: Arc<LogStore>,
    metrics: Arc<MetricsStore>,
) -> Handle {
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(actor::run(name, config, rx, event_tx, log_store, metrics));
    Handle { tx }
}

// ---------------------------------------------------------------------------
// Defaults (matching Pebble)
// ---------------------------------------------------------------------------

pub(super) const DEFAULT_KILL_DELAY: Duration = Duration::from_secs(5);
pub(super) const DEFAULT_BACKOFF_DELAY: Duration = Duration::from_millis(500);
pub(super) const DEFAULT_BACKOFF_FACTOR: f64 = 2.0;
pub(super) const DEFAULT_BACKOFF_LIMIT: Duration = Duration::from_secs(30);
