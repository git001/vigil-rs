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

use super::{Cmd, DEFAULT_BACKOFF_DELAY, Event, EventKind, Snapshot};

mod backoff;
mod handlers;
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
    backoff_count: u32,
    current_backoff: Duration,
    event_tx: mpsc::Sender<Event>,
    log_store: Arc<LogStore>,
    metrics: Arc<MetricsStore>,
}

impl Actor {
    pub(super) fn new(
        name: String,
        config: ServiceConfig,
        event_tx: mpsc::Sender<Event>,
        log_store: Arc<LogStore>,
        metrics: Arc<MetricsStore>,
    ) -> Self {
        Actor {
            name,
            config,
            state: ServiceState::Inactive,
            since: Utc::now(),
            child: None,
            backoff_count: 0,
            current_backoff: DEFAULT_BACKOFF_DELAY,
            event_tx,
            log_store,
            metrics,
        }
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        Snapshot {
            name: self.name.clone(),
            state: self.state,
            since: self.since,
            startup: self.config.startup,
            pid: self.child.as_ref().and_then(|c| c.id()),
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
    event_tx: mpsc::Sender<Event>,
    log_store: Arc<LogStore>,
    metrics: Arc<MetricsStore>,
) {
    let mut actor = Actor::new(
        name.clone(),
        config,
        event_tx,
        Arc::clone(&log_store),
        metrics,
    );
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
                }
            }

            status = async {
                match actor.child.as_mut() {
                    Some(c) => c.wait().await,
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
        }
    }

    for h in push_tasks {
        h.abort();
    }
    actor.cleanup().await;
    debug!(service = %name, "actor shut down");
}
