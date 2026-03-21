// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Command handlers — Start, Stop, Restart, child exit, kill deadline, backoff.

use std::pin::Pin;
use std::time::Duration;

use tokio::time::Sleep;
use tracing::{error, info, warn};
use vigil_types::plan::OnExit;

use crate::state::ServiceState;

use super::{Actor, Event, EventKind};

impl Actor {
    // -----------------------------------------------------------------------
    // Command handlers
    // -----------------------------------------------------------------------

    pub(super) async fn handle_start(
        &mut self,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
        stop_deadline: &mut Option<Pin<Box<Sleep>>>,
    ) {
        if self.state.is_running() {
            let _ = reply.send(Ok(()));
            return;
        }
        if self.state == ServiceState::Stopping {
            let _ = reply.send(Err(anyhow::anyhow!(
                "service '{}' is stopping, wait before starting",
                self.name
            )));
            return;
        }

        self.transition(ServiceState::Starting).await;
        match self.do_start().await {
            Ok(()) => {
                self.reset_backoff();
                *stop_deadline = None;
                let _ = reply.send(Ok(()));
            }
            Err(e) => {
                error!(service = %self.name, error = %e, "failed to start");
                self.transition(ServiceState::Error).await;
                let _ = reply.send(Err(e));
            }
        }
    }

    pub(super) async fn handle_stop(
        &mut self,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
        stop_deadline: &mut Option<Pin<Box<Sleep>>>,
        backoff_sleep: &mut Option<Pin<Box<Sleep>>>,
    ) {
        match self.state {
            ServiceState::Inactive | ServiceState::Error | ServiceState::Stopping => {
                let _ = reply.send(Ok(()));
                return;
            }
            ServiceState::Backoff => {
                *backoff_sleep = None;
                self.transition(ServiceState::Inactive).await;
                let _ = reply.send(Ok(()));
                return;
            }
            ServiceState::Starting | ServiceState::Active => {}
        }

        self.send_stop_signal();
        self.transition(ServiceState::Stopping).await;
        *stop_deadline = Some(Box::pin(tokio::time::sleep(self.kill_delay())));
        let _ = reply.send(Ok(()));
    }

    pub(super) async fn handle_restart(
        &mut self,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
        backoff_sleep: &mut Option<Pin<Box<Sleep>>>,
        stop_deadline: &mut Option<Pin<Box<Sleep>>>,
        pending_restart: &mut bool,
    ) {
        if self.state.is_running() || self.state == ServiceState::Stopping {
            self.send_stop_signal();
            self.transition(ServiceState::Stopping).await;
            *stop_deadline = Some(Box::pin(tokio::time::sleep(self.kill_delay())));
            *backoff_sleep = None;
            *pending_restart = true;
            let _ = reply.send(Ok(()));
        } else {
            // Service is not running — start directly; no deferred restart needed.
            let (tx2, rx2) = tokio::sync::oneshot::channel();
            self.handle_start(tx2, stop_deadline).await;
            let res = rx2.await.unwrap_or(Ok(()));
            let _ = reply.send(res);
        }
    }

    pub(super) async fn handle_child_exit(
        &mut self,
        exit: std::io::Result<std::process::ExitStatus>,
        backoff_sleep: &mut Option<Pin<Box<Sleep>>>,
        stop_deadline: &mut Option<Pin<Box<Sleep>>>,
        pending_restart: &mut bool,
    ) {
        let exit_status = exit.as_ref().ok().copied();
        let success = exit_status.map(|s| s.success()).unwrap_or(false);
        let raw_exit_code =
            exit_status
                .and_then(|s| s.code())
                .unwrap_or(if success { 0 } else { 1 });

        let _ = self
            .event_tx
            .send(Event {
                service: self.name.clone(),
                kind: EventKind::ProcessExited { success },
            })
            .await;

        info!(service = %self.name, success, exit_code = raw_exit_code, "process exited");
        self.child = None;
        *stop_deadline = None;

        if self.state == ServiceState::Stopping {
            self.transition(ServiceState::Inactive).await;
            if *pending_restart {
                *pending_restart = false;
                self.transition(ServiceState::Starting).await;
                match self.do_start().await {
                    Ok(()) => self.reset_backoff(),
                    Err(e) => {
                        error!(service = %self.name, error = %e, "restart failed");
                        self.transition(ServiceState::Error).await;
                    }
                }
            }
            return;
        }

        let policy = if success {
            self.config.on_success
        } else {
            self.config.on_failure
        };

        match policy {
            Some(OnExit::Ignore) => {
                self.transition(ServiceState::Inactive).await;
            }
            Some(OnExit::Shutdown) => {
                // success → 0, failure → 10 (matches documented behaviour)
                let exit_code = if success { 0 } else { 10 };
                info!(service = %self.name, exit_code, "shutdown requested");
                self.transition(ServiceState::Inactive).await;
                let _ = self
                    .event_tx
                    .send(Event {
                        service: self.name.clone(),
                        kind: EventKind::DaemonShutdown { exit_code },
                    })
                    .await;
            }
            Some(OnExit::FailureShutdown) => {
                // Always exits with 10 regardless of how the process exited
                info!(service = %self.name, "failure-shutdown requested");
                self.transition(ServiceState::Inactive).await;
                let _ = self
                    .event_tx
                    .send(Event {
                        service: self.name.clone(),
                        kind: EventKind::DaemonShutdown { exit_code: 10 },
                    })
                    .await;
            }
            Some(OnExit::SuccessShutdown) => {
                // Always exits with 0 regardless of how the process exited
                info!(service = %self.name, "success-shutdown requested");
                self.transition(ServiceState::Inactive).await;
                let _ = self
                    .event_tx
                    .send(Event {
                        service: self.name.clone(),
                        kind: EventKind::DaemonShutdown { exit_code: 0 },
                    })
                    .await;
            }
            Some(OnExit::Restart) | None => {
                if self.backoff_limit_exceeded() {
                    error!(service = %self.name, "backoff limit exceeded, giving up");
                    self.transition(ServiceState::Error).await;
                } else {
                    let delay = self.next_backoff();
                    info!(service = %self.name, ?delay, "scheduling restart (backoff)");
                    self.transition(ServiceState::Backoff).await;
                    *backoff_sleep = Some(Box::pin(tokio::time::sleep(delay)));
                }
            }
        }
    }

    pub(super) async fn handle_kill_deadline(&mut self) {
        if self.child.is_some() {
            warn!(service = %self.name, "kill-delay expired, sending SIGKILL");
            self.send_sigkill();
        }
    }

    pub(super) async fn handle_backoff_expired(
        &mut self,
        stop_deadline: &mut Option<Pin<Box<Sleep>>>,
    ) {
        info!(service = %self.name, "backoff elapsed, restarting");
        self.transition(ServiceState::Starting).await;
        match self.do_start().await {
            Ok(()) => {}
            Err(e) => {
                error!(service = %self.name, error = %e, "restart after backoff failed");
                self.transition(ServiceState::Error).await;
            }
        }
        *stop_deadline = None;
    }

    pub(super) async fn cleanup(&mut self) {
        if self.child.is_some() {
            self.send_stop_signal();
            tokio::time::sleep(Duration::from_millis(500)).await;
            self.send_sigkill();
            if let Some(mut c) = self.child.take() {
                let _ = c.wait().await;
            }
        }
    }
}
