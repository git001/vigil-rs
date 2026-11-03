// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::process::Child;
use tokio::sync::mpsc;
use tokio::time::Sleep;
use tracing::debug;
use vigil_types::plan::ServiceConfig;

use crate::logs::LogStore;
use crate::metrics::MetricsStore;
use crate::state::ServiceState;

use super::notify::{NotifyMsg, NotifySocket};
use super::{Cmd, Context, DEFAULT_BACKOFF_DELAY, Event, EventKind, ExitCodeMode, Snapshot};

mod backoff;
mod handlers;
mod ready;
mod signals;
mod spawn;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Actor
// ---------------------------------------------------------------------------

pub(super) struct Actor {
    name: String,
    config: ServiceConfig,
    state: ServiceState,
    since: DateTime<Utc>,
    child: Option<Child>,
    /// Taken right before the current child was spawned (see
    /// `reaper::wait_child`).
    spawned_at: std::time::Instant,
    backoff_count: u32,
    current_backoff: Duration,
    event_tx: mpsc::Sender<Event>,
    log_store: Arc<LogStore>,
    metrics: Arc<MetricsStore>,
    /// How vigild's exit code is chosen on a shutdown policy.
    exit_code_mode: ExitCodeMode,
    /// How the last process ended.
    last_exit: Option<vigil_types::api::ExitInfo>,
    /// `<path-prefix>/notify`, where `ready: notify` sockets are created.
    notify_dir: std::path::PathBuf,
    /// Notify socket of the current start (`ready: notify` only).
    notify: Option<NotifySocket>,
    /// Incremented for every notify socket; messages from older sockets
    /// are ignored.
    notify_generation: u64,
    notify_tx: mpsc::Sender<NotifyMsg>,
    /// Taken by `run`; `None` afterwards.
    notify_rx: Option<mpsc::Receiver<NotifyMsg>>,
    /// UID the current process runs as (for notify sender checks).
    service_uid: u32,
    /// Deadline for reaching readiness while `Starting`.
    ready_deadline: Option<tokio::time::Instant>,
    /// Readiness timed out; the process is being stopped and its exit
    /// counts as a failure.
    ready_failed: bool,
    /// Last `STATUS=` text from sd_notify.
    status_message: Option<String>,
}

impl Actor {
    pub(super) fn new(name: String, config: ServiceConfig, ctx: Context) -> Self {
        let Context {
            event_tx,
            log_store,
            metrics,
            exit_code_mode,
            notify_dir,
        } = ctx;
        let (notify_tx, notify_rx) = mpsc::channel(16);
        Actor {
            name,
            config,
            state: ServiceState::Inactive,
            since: Utc::now(),
            child: None,
            spawned_at: std::time::Instant::now(),
            backoff_count: 0,
            current_backoff: DEFAULT_BACKOFF_DELAY,
            event_tx,
            log_store,
            metrics,
            exit_code_mode,
            last_exit: None,
            notify_dir,
            notify: None,
            notify_generation: 0,
            notify_tx,
            notify_rx: Some(notify_rx),
            service_uid: nix::unistd::Uid::effective().as_raw(),
            ready_deadline: None,
            ready_failed: false,
            status_message: None,
        }
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        Snapshot {
            name: self.name.clone(),
            state: self.state,
            since: self.since,
            startup: self.config.startup,
            pid: self.child.as_ref().and_then(|c| c.id()),
            last_exit: self.last_exit,
            status_message: self.status_message.clone(),
        }
    }

    pub(super) async fn transition(&mut self, new_state: ServiceState) {
        debug!(service = %self.name, ?new_state, "state transition");
        self.state = new_state;
        self.since = Utc::now();
        self.metrics
            .set_service_active(&self.name, new_state == ServiceState::Active);
        let _ = self
            .event_tx
            .send(Event {
                service: self.name.clone(),
                kind: EventKind::StateChanged { new_state },
            })
            .await;
    }
}

// ---------------------------------------------------------------------------
// Main actor loop
// ---------------------------------------------------------------------------

pub(super) async fn run(
    name: String,
    config: ServiceConfig,
    mut rx: mpsc::Receiver<Cmd>,
    ctx: Context,
) {
    let log_store = Arc::clone(&ctx.log_store);
    let mut actor = Actor::new(name.clone(), config, ctx);
    let mut notify_rx = actor.notify_rx.take().expect("fresh actor");
    let mut backoff_sleep: Option<Pin<Box<Sleep>>> = None;
    let mut stop_deadline: Option<Pin<Box<Sleep>>> = None;
    let mut pending_restart = false;

    // Spawn log-push tasks if the service config requests it.
    // Each task connects to the target with exponential-backoff retry and
    // streams ndjson log entries for this service. Handles are aborted when
    // the actor exits so the tasks stop immediately on shutdown / replan.
    let mut push_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    if let Some(path) = actor.config.logs_push_socket.clone() {
        push_tasks.push(crate::logs::spawn_push_unix(
            name.clone(),
            path,
            Arc::clone(&log_store),
        ));
    }
    if let Some(addr) = actor.config.logs_push_addr.clone() {
        push_tasks.push(crate::logs::spawn_push_tcp(
            name.clone(),
            addr,
            Arc::clone(&log_store),
        ));
    }

    loop {
        let ready_deadline = actor.ready_deadline;
        tokio::select! {
            biased;

            cmd = rx.recv() => {
                match cmd {
                    None | Some(Cmd::Shutdown) => break,
                    Some(Cmd::Start(reply)) => {
                        actor.handle_start(reply, &mut stop_deadline).await;
                    }
                    Some(Cmd::Stop(reply)) => {
                        actor.handle_stop(reply, &mut stop_deadline, &mut backoff_sleep).await;
                    }
                    Some(Cmd::Restart(reply)) => {
                        actor.handle_restart(reply, &mut backoff_sleep, &mut stop_deadline, &mut pending_restart).await;
                    }
                    Some(Cmd::Status(reply)) => {
                        let _ = reply.send(actor.snapshot());
                    }
                    Some(Cmd::ForwardSignal(sig)) => {
                        actor.send_signal(sig);
                    }
                    Some(Cmd::CheckPassed { check, started_at }) => {
                        actor.handle_check_passed(&check, started_at).await;
                    }
                }
            }

            Some(msg) = notify_rx.recv() => {
                actor.handle_notify(msg).await;
            }

            status = async {
                match actor.child.as_mut() {
                    Some(c) => crate::reaper::wait_child(c, actor.spawned_at).await,
                    None => std::future::pending().await,
                }
            } => {
                actor.handle_child_exit(status, &mut backoff_sleep, &mut stop_deadline, &mut pending_restart).await;
            }

            _ = async {
                match stop_deadline.as_mut() {
                    Some(s) => s.await,
                    None => std::future::pending().await,
                }
            }, if stop_deadline.is_some() => {
                actor.handle_kill_deadline().await;
                stop_deadline = None;
            }

            _ = async {
                match backoff_sleep.as_mut() {
                    Some(s) => s.await,
                    None => std::future::pending().await,
                }
            }, if backoff_sleep.is_some() => {
                actor.handle_backoff_expired(&mut stop_deadline).await;
                backoff_sleep = None;
            }

            _ = async {
                match ready_deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending().await,
                }
            }, if ready_deadline.is_some() => {
                actor.handle_ready_timeout(&mut stop_deadline).await;
            }
        }
    }

    for h in push_tasks {
        h.abort();
    }
    actor.cleanup().await;
    debug!(service = %name, "actor shut down");
}
