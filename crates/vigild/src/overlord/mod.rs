// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The overlord: one task that owns the plan and the service and check
//! actors, and answers the API.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use tokio::sync::mpsc;
use vigil_types::api::ChangeInfo;
use vigil_types::plan::Plan;

use crate::alert::AlertSender;
use crate::check::{self, CheckEvent};
use crate::identity::IdentityStore;
use crate::logs::LogStore;
use crate::metrics::MetricsStore;
use crate::service;

mod actions;
mod autostart;
mod check_events;
mod cmd;
mod events;
mod handlers;
mod health_metric;
mod lifecycle;
pub mod plan;
mod queries;
mod restart;
mod spawn;
mod stop_order;
mod sync;
#[cfg(test)]
mod tests;
pub mod watch_layers;

pub use cmd::{Cmd, Handle, SignalError};
pub use spawn::{Spawned, spawn};

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

pub(super) struct ServiceEntry {
    pub(super) handle: service::Handle,
    pub(super) snapshot: service::Snapshot,
}

pub(super) struct CheckEntry {
    pub(super) handle: check::Handle,
    pub(super) config: vigil_types::plan::CheckConfig,
    /// Config of the exec check's `service-context` service when the actor
    /// was spawned; a change restarts the check like a change of its own.
    pub(super) context: Option<vigil_types::plan::ServiceConfig>,
}

pub(super) struct Overlord {
    pub(super) plan: Plan,
    pub(super) services: IndexMap<String, ServiceEntry>,
    pub(super) checks: IndexMap<String, CheckEntry>,
    /// Clients of the HTTP checks, built when the plan was loaded.
    pub(super) check_clients: crate::check::PreparedChecks,
    pub(super) alert_sender: AlertSender,
    pub(super) changes: Vec<ChangeInfo>,
    pub(super) change_counter: u64,
    pub(super) boot_id: String,
    pub(super) start_time: DateTime<Utc>,
    pub(super) http_address: String,
    pub(super) https_address: Option<String>,
    pub(super) log_store: Arc<LogStore>,
    pub(super) metrics: Arc<MetricsStore>,
    pub(super) event_rx: mpsc::Receiver<service::Event>,
    pub(super) event_tx: mpsc::Sender<service::Event>,
    pub(super) check_event_rx: mpsc::Receiver<CheckEvent>,
    pub(super) check_event_tx: mpsc::Sender<CheckEvent>,
    pub(super) cmd_rx: mpsc::Receiver<Cmd>,
    pub(super) layers_dir: std::path::PathBuf,
    /// Services with `startup: enabled` that are waiting for their `after:`
    /// dependencies to start before being auto-started.
    pub(super) pending_autostart: Vec<String>,
    /// Services a replan stopped to start them again with their new config
    /// (see `restart.rs`).
    pub(super) pending_restart: Vec<String>,
    /// How vigild's exit code is chosen on a shutdown policy (`--exit-code`).
    pub(super) exit_code_mode: service::ExitCodeMode,
    /// `<path-prefix>/notify` for `ready: notify` sockets.
    pub(super) notify_dir: PathBuf,
    /// Services that are healthy (`active` for `healthy-time`, or a
    /// successful oneshot run). Checks of other services stay quiet.
    pub(super) healthy: std::collections::HashSet<String>,
    /// Shared with the API server; the layer identities are replaced on
    /// every successful load.
    pub(super) identity_store: Arc<IdentityStore>,
    /// The files the layers refer to (see [`plan::Loaded::files`]), watched
    /// by `--watch-layers` too.
    pub(super) watched_files: watch_layers::WatchedFiles,
}
