// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::process::Child;
use tokio::sync::mpsc;
use tracing::debug;
use vigil_types::plan::ServiceConfig;

use crate::logs::LogStore;
use crate::metrics::MetricsStore;
use crate::state::ServiceState;

use super::notify::{NotifyMsg, NotifySocket};
use super::{Cmd, Context, DEFAULT_BACKOFF_DELAY, Event, EventKind, ExitCodeMode};

mod backoff;
mod handlers;
mod health;
mod oneshot;
mod ready;
mod signals;
mod spawn;
mod watch;

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
    /// Armed cron schedule (`schedule` with `startup: enabled`).
    schedule: Option<crate::service::schedule::Schedule>,
    /// Next scheduled run.
    next_run: Option<jiff::Timestamp>,
    /// Files watched for the running process (`watch-files`).
    watch: Option<crate::service::watch::FileWatch>,
    /// Next poll of `watch`.
    watch_deadline: Option<tokio::time::Instant>,
    /// When the current `active` period counts as healthy (`healthy-time`).
    healthy_deadline: Option<tokio::time::Instant>,
    /// SIGKILL after the stop signal (`kill-delay`).
    stop_deadline: Option<tokio::time::Instant>,
    /// Next restart after a failure (backoff).
    backoff_deadline: Option<tokio::time::Instant>,
    /// Start again once the stopping process is gone (`restart`).
    pending_restart: bool,
}

/// An optional duration field of the config, or `default` when unset. The
/// plan check rejects invalid values; one that slipped through also gets the
/// default.
fn dur(value: &Option<String>, default: Duration) -> Duration {
    value
        .as_deref()
        .and_then(|s| crate::duration::parse_duration(s).ok())
        .unwrap_or(default)
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
            watch: None,
            watch_deadline: None,
            healthy_deadline: None,
            stop_deadline: None,
            backoff_deadline: None,
            pending_restart: false,
            ready_failed: false,
            status_message: None,
            schedule: None,
            next_run: None,
        }
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> super::Snapshot {
        super::Snapshot {
            name: self.name.clone(),
            state: self.state,
            since: self.since,
            startup: self.config.startup(),
            last_exit: self.last_exit,
            status_message: self.status_message.clone(),
            next_run: self
                .next_run
                .and_then(|t| DateTime::from_timestamp(t.as_second(), 0)),
        }
    }

    /// Report `kind` to the overlord.
    pub(super) async fn send_event(&self, kind: EventKind) {
        let _ = self
            .event_tx
            .send(Event {
                service: self.name.clone(),
                kind,
            })
            .await;
    }

    pub(super) async fn transition(&mut self, new_state: ServiceState) {
        debug!(service = %self.name, ?new_state, "state transition");
        self.state = new_state;
        self.since = Utc::now();
        self.arm_healthy_deadline();
        self.metrics
            .set_service_active(&self.name, new_state == ServiceState::Active);
        self.metrics.set_service_running(
            &self.name,
            matches!(
                new_state,
                ServiceState::Starting | ServiceState::Active | ServiceState::Stopping
            ),
        );
        self.send_event(EventKind::StateChanged { new_state }).await;
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
    actor.arm_schedule().await;

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
        // Copies: the arms below borrow the actor mutably.
        let stop_deadline = actor.stop_deadline;
        let backoff_deadline = actor.backoff_deadline;
        let ready_deadline = actor.ready_deadline;
        let watch_deadline = actor.watch_deadline;
        let healthy_deadline = actor.healthy_deadline;
        let next_run = actor.next_run_deadline();
        tokio::select! {
            biased;

            cmd = rx.recv() => {
                match cmd {
                    None | Some(Cmd::Shutdown) => break,
                    Some(Cmd::Start(reply)) => {
                        let _ = reply.send(actor.try_start().await);
                    }
                    Some(Cmd::Stop(reply)) => {
                        actor.stop().await;
                        let _ = reply.send(Ok(()));
                    }
                    Some(Cmd::Restart(reply)) => {
                        let _ = reply.send(actor.restart().await);
                    }
                    #[cfg(test)]
                    Some(Cmd::Status(reply)) => {
                        let _ = reply.send(actor.snapshot());
                    }
                    Some(Cmd::ForwardSignal(sig)) => {
                        actor.send_signal(sig);
                    }
                    Some(Cmd::UpdateConfig(config)) => {
                        actor.handle_update_config(*config).await;
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
                actor.handle_child_exit(status).await;
            }

            _ = sleep_opt(stop_deadline), if stop_deadline.is_some() => {
                actor.stop_deadline = None;
                actor.handle_kill_deadline().await;
            }

            _ = sleep_opt(backoff_deadline), if backoff_deadline.is_some() => {
                actor.backoff_deadline = None;
                actor.handle_backoff_expired().await;
            }

            _ = sleep_opt(ready_deadline), if ready_deadline.is_some() => {
                actor.handle_ready_timeout().await;
            }

            _ = sleep_opt(watch_deadline), if watch_deadline.is_some() => {
                actor.handle_watch_tick().await;
            }

            _ = sleep_opt(healthy_deadline), if healthy_deadline.is_some() => {
                actor.report_healthy().await;
            }

            _ = sleep_opt(next_run), if next_run.is_some() => {
                actor.handle_schedule_due().await;
            }
        }
    }

    for h in push_tasks {
        h.abort();
    }
    actor.cleanup().await;
    debug!(service = %name, "actor shut down");
}

/// Sleep until `deadline`; never returns without one (for `select!` arms
/// guarded by `deadline.is_some()`).
async fn sleep_opt(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}
