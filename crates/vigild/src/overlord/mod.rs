// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::path::PathBuf;
use std::process;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use tokio::sync::{mpsc, oneshot};
use tracing::info;
use uuid::Uuid;
use vigil_types::api::{AlertInfo, ChangeInfo, CheckInfo, ServiceInfo, SystemInfo};
use vigil_types::plan::Plan;

use crate::alert::AlertSender;
use crate::check::{self, CheckEvent};
use crate::identity::IdentityStore;
use crate::logs::LogStore;
use crate::metrics::MetricsStore;
use crate::service;

mod events;
mod handlers;
mod lifecycle;
mod restart;
pub mod plan;
#[cfg(test)]
mod tests;
pub mod watch_layers;

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

pub enum Cmd {
    Services {
        action: vigil_types::api::ServiceAction,
        names: Vec<String>,
        reply: oneshot::Sender<anyhow::Result<ChangeInfo>>,
    },
    GetServices {
        names: Vec<String>,
        reply: oneshot::Sender<Vec<ServiceInfo>>,
    },
    GetChanges {
        id: Option<String>,
        reply: oneshot::Sender<Vec<ChangeInfo>>,
    },
    GetChecks {
        names: Vec<String>,
        reply: oneshot::Sender<Vec<CheckInfo>>,
    },
    /// Run a check now (`POST /v1/checks/refresh`). `None`: no check of
    /// that name in the plan. The overlord only hands the request on; the
    /// reply comes from the check, so a slow probe never blocks it.
    RefreshCheck {
        name: String,
        reply: oneshot::Sender<Option<vigil_types::api::CheckRefresh>>,
    },
    /// `GET /v1/health`: whether no selected, running check is down. `Err`
    /// names a check that is not in the plan.
    Health {
        level: Option<vigil_types::plan::CheckLevel>,
        names: Vec<String>,
        reply: oneshot::Sender<Result<bool, String>>,
    },
    GetAlerts {
        names: Vec<String>,
        reply: oneshot::Sender<Vec<AlertInfo>>,
    },
    GetSystemInfo {
        reply: oneshot::Sender<SystemInfo>,
    },
    ReloadLayers {
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    /// Send `signal` to the process groups of `services`
    /// (`POST /v1/signals`). All of them must exist and run, otherwise
    /// none gets it. Ok: the services that got it.
    Signal {
        signal: nix::sys::signal::Signal,
        services: Vec<String>,
        reply: oneshot::Sender<Result<Vec<String>, SignalError>>,
    },
    /// Forward a signal to all currently-running service process groups.
    ForwardSignal {
        signal: nix::sys::signal::Signal,
    },
    Shutdown,
}

/// Why `POST /v1/signals` sent nothing (or not to every service).
#[derive(Debug, PartialEq)]
pub enum SignalError {
    /// No service named in the request (400).
    NoServices,
    /// A name that is not a service (404).
    NotFound(String),
    /// Services without a running process (409).
    NotRunning(Vec<String>),
}

impl std::fmt::Display for SignalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoServices => write!(f, "services must name at least one service"),
            Self::NotFound(name) => write!(f, "service {name:?} not found"),
            Self::NotRunning(names) => {
                let list: Vec<String> = names.iter().map(|n| format!("{n:?}")).collect();
                write!(f, "not running: {}", list.join(", "))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Handle
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Handle {
    pub tx: mpsc::Sender<Cmd>,
}

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

// ---------------------------------------------------------------------------
// Spawn
// ---------------------------------------------------------------------------

/// What [`spawn`] hands to the API server.
pub struct Spawned {
    pub handle: Handle,
    pub log_store: Arc<LogStore>,
    pub metrics: Arc<MetricsStore>,
    /// Identities from the layer files (kept current by every replan) and
    /// those added over the API.
    pub identity_store: Arc<IdentityStore>,
    pub task: tokio::task::JoinHandle<()>,
}

pub async fn spawn(
    layers_dir: PathBuf,
    http_address: String,
    https_address: Option<String>,
    log_buffer: usize,
    exit_code_mode: service::ExitCodeMode,
    notify_dir: PathBuf,
    watch_layers: Option<std::time::Duration>,
) -> anyhow::Result<Spawned> {
    let (cmd_tx, cmd_rx) = mpsc::channel(128);
    let (event_tx, event_rx) = mpsc::channel(256);
    let (check_event_tx, check_event_rx) = mpsc::channel(256);
    // Broadcast capacity: half the ring buffer, clamped to [64, 4096].
    // Large enough to absorb brief consumer stalls without dropping entries;
    // small enough that a permanently-disconnected follower doesn't hold
    // unbounded memory.
    let broadcast_capacity = (log_buffer / 2).clamp(64, 4096);
    let log_store = LogStore::new(log_buffer, broadcast_capacity);
    let metrics = MetricsStore::new();
    // Off the async runtime like every replan: a hanging file system (a
    // dead NFS mount behind a `tls:` file) ends the start with an error
    // instead of blocking it.
    let dir = layers_dir.clone();
    let plan::Loaded {
        plan: plan_val,
        alerts,
        checks: check_clients,
        identities,
        files,
    } = crate::template::off_runtime("layers", move || plan::load(&dir)).await?;
    let identity_store = IdentityStore::with_layer(identities);
    let watched_files = watch_layers::WatchedFiles::new(files);

    let (queue_depth, max_age) = crate::alert::queue_limits(&plan_val);
    let mut alert_sender = AlertSender::with_queue_limits(queue_depth, max_age);
    alert_sender.update_alerts(alerts);
    alert_sender.spawn_worker();

    let ov = Overlord {
        plan: plan_val,
        services: IndexMap::new(),
        checks: IndexMap::new(),
        check_clients,
        alert_sender,
        changes: Vec::new(),
        change_counter: 0,
        boot_id: Uuid::new_v4().to_string(),
        start_time: Utc::now(),
        http_address,
        https_address,
        log_store: Arc::clone(&log_store),
        metrics: Arc::clone(&metrics),
        event_rx,
        event_tx,
        check_event_rx,
        check_event_tx,
        cmd_rx,
        layers_dir: layers_dir.clone(),
        pending_autostart: Vec::new(),
        pending_restart: Vec::new(),
        exit_code_mode,
        notify_dir,
        healthy: Default::default(),
        identity_store: Arc::clone(&identity_store),
        watched_files: watched_files.clone(),
    };

    let task = tokio::spawn(run(ov));
    if let Some(interval) = watch_layers {
        // Ends by itself when the overlord's command channel closes.
        watch_layers::spawn(layers_dir, watched_files, interval, cmd_tx.clone());
    }
    Ok(Spawned {
        handle: Handle { tx: cmd_tx },
        log_store,
        metrics,
        identity_store,
        task,
    })
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

impl Overlord {
    /// Stop everything and exit vigild with `code`. The single place where a
    /// policy (service exit or check failure) terminates the daemon.
    pub(super) async fn shutdown(&mut self, code: i32) -> ! {
        info!(exit_code = code, "shutting down on policy");
        self.stop_all().await;
        process::exit(code);
    }
}

async fn run(mut ov: Overlord) {
    ov.sync_actors();
    ov.autostart().await;

    loop {
        tokio::select! {
            biased;

            cmd = ov.cmd_rx.recv() => match cmd {
                None | Some(Cmd::Shutdown) => break,
                Some(cmd) => ov.handle_cmd(cmd).await,
            },

            event = ov.event_rx.recv() => {
                if let Some(ev) = event
                    && let Some(exit_code) = ov.handle_svc_event(ev).await
                {
                    ov.shutdown(exit_code).await;
                }
            },

            event = ov.check_event_rx.recv() => {
                if let Some(ev) = event { ov.handle_check_event(ev).await; }
            },
        }
    }

    ov.stop_all().await;
    info!("overlord shut down");
}
