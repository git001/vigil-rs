// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Command handlers — Start, Stop, Restart, child exit, kill deadline, backoff.

use std::pin::Pin;
use std::time::Duration;

use tokio::time::Sleep;
use tracing::{error, info, warn};
use vigil_types::plan::OnExit;

use crate::state::ServiceState;

use super::super::{effective_on_failure, effective_on_success, shutdown_code};
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
            ServiceState::Inactive
            | ServiceState::Error
            | ServiceState::Stopping
            | ServiceState::Done => {
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
        use std::os::unix::process::ExitStatusExt;

        let exit_info = exit.as_ref().ok().map(|s| vigil_types::api::ExitInfo {
            code: s.code(),
            signal: s.signal(),
        });
        // Shell-style code (128 + signal for signals), then exit-code-map.
        let raw_code = exit_info.and_then(|e| e.shell_code());
        let mut code = raw_code.map(|c| self.config.exit_code_map.get(&c).copied().unwrap_or(c));
        let mut success = match code {
            Some(c) if self.config.success_exit_codes.is_empty() => c == 0,
            Some(c) => self.config.success_exit_codes.contains(&c),
            None => false,
        };
        // Stopped because it never became ready: always a failure, and the
        // stop signal's code (e.g. 143) says nothing about the service.
        if std::mem::take(&mut self.ready_failed) {
            success = false;
            code = None;
        }
        self.ready_deadline = None;
        self.notify = None;
        self.stop_watching();
        self.last_exit = exit_info;
        self.metrics
            .set_service_last_exit_code(&self.name, raw_code);

        let _ = self
            .event_tx
            .send(Event {
                service: self.name.clone(),
                kind: EventKind::ProcessExited {
                    success,
                    exit: exit_info,
                },
            })
            .await;

        info!(
            service = %self.name,
            success,
            exit_code = ?exit_info.and_then(|e| e.code),
            signal = ?exit_info.and_then(|e| e.signal),
            mapped_code = ?code,
            "process exited"
        );
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

        if self.is_oneshot() {
            self.metrics.record_oneshot_run(&self.name, success);
        }
        let policy = if success {
            effective_on_success(&self.config)
        } else {
            Some(effective_on_failure(&self.config))
        };

        match policy {
            // A finished oneshot: `done` (dependents may start now).
            None => {
                self.transition(ServiceState::Done).await;
            }
            Some(OnExit::Ignore) if success && self.is_oneshot() => {
                self.transition(ServiceState::Done).await;
            }
            Some(OnExit::Ignore) => {
                self.transition(ServiceState::Inactive).await;
            }
            Some(
                policy @ (OnExit::Shutdown | OnExit::FailureShutdown | OnExit::SuccessShutdown),
            ) => {
                let exit_code = shutdown_code(self.exit_code_mode, policy, success, code)
                    .expect("shutdown policies always yield a code");
                info!(service = %self.name, ?policy, exit_code, "daemon shutdown requested");
                self.transition(ServiceState::Inactive).await;
                let _ = self
                    .event_tx
                    .send(Event {
                        service: self.name.clone(),
                        kind: EventKind::DaemonShutdown { exit_code },
                    })
                    .await;
            }
            Some(OnExit::Restart) => {
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

    pub(super) async fn handle_update_config(&mut self, config: vigil_types::plan::ServiceConfig) {
        let old = serde_json::to_string(&self.config).unwrap_or_default();
        let new = serde_json::to_string(&config).unwrap_or_default();
        if old == new {
            return;
        }
        info!(service = %self.name, "config changed, applies from the next start");
        self.config = config;
        self.arm_schedule().await;
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
        self.notify = None;
        self.stop_watching();
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
