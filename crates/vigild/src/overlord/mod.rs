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
use crate::logs::LogStore;
use crate::metrics::MetricsStore;
use crate::service;

mod events;
mod handlers;
mod lifecycle;
pub mod plan;
#[cfg(test)]
mod tests;

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
    /// Forward a signal to all currently-running service process groups.
    ForwardSignal {
        signal: nix::sys::signal::Signal,
    },
    Shutdown,
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
}

pub(super) struct Overlord {
    pub(super) plan: Plan,
    pub(super) services: IndexMap<String, ServiceEntry>,
    pub(super) checks: IndexMap<String, CheckEntry>,
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
}

// ---------------------------------------------------------------------------
// Spawn
// ---------------------------------------------------------------------------

#[allow(clippy::type_complexity)]
pub fn spawn(
    layers_dir: PathBuf,
    http_address: String,
    https_address: Option<String>,
    log_buffer: usize,
) -> anyhow::Result<(
    Handle,
    Arc<LogStore>,
    Arc<MetricsStore>,
    tokio::task::JoinHandle<()>,
)> {
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
    let plan_val = plan::load_plan(&layers_dir)?;

    let queue_depth = plan_val
        .alert_queue_depth
        .unwrap_or(crate::alert::DEFAULT_DELIVERY_QUEUE);
    let max_age = plan_val
        .alert_max_queue_time
        .as_deref()
        .and_then(|s| crate::duration::parse_duration(s).ok())
        .unwrap_or(crate::alert::DEFAULT_DELIVERY_AGE);
    let mut alert_sender = AlertSender::with_queue_limits(queue_depth, max_age);
    alert_sender.update_alerts(plan_val.alerts.clone());
    alert_sender.spawn_worker();

    let ov = Overlord {
        plan: plan_val,
        services: IndexMap::new(),
        checks: IndexMap::new(),
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
        layers_dir,
        pending_autostart: Vec::new(),
    };

    let join = tokio::spawn(run(ov));
    Ok((Handle { tx: cmd_tx }, log_store, metrics, join))
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

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
                    ov.stop_all().await;
                    process::exit(exit_code);
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
