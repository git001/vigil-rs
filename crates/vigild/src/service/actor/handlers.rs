// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Command handlers — Start, Stop, Restart, child exit, kill deadline, backoff.

use std::time::Duration;

use tokio::time::Instant;
use tracing::{error, info, warn};
use vigil_types::plan::OnExit;

use crate::state::ServiceState;

use super::super::{effective_on_failure, effective_on_success, shutdown_code};
use super::{Actor, EventKind};

impl Actor {
    // -----------------------------------------------------------------------
    // Command handlers
    // -----------------------------------------------------------------------

    /// `vigil start`: start unless it already runs. Errors while stopping.
    pub(super) async fn try_start(&mut self) -> anyhow::Result<()> {
        if self.state.is_running() {
            return Ok(());
        }
        anyhow::ensure!(
            self.state != ServiceState::Stopping,
            "service '{}' is stopping, wait before starting",
            self.name
        );
        self.start_now("failed to start").await?;
        self.reset_backoff();
        Ok(())
    }

    /// `vigil stop`.
    pub(super) async fn stop(&mut self) {
        match self.state {
            ServiceState::Inactive
            | ServiceState::Error
            | ServiceState::Stopping
            | ServiceState::Done => {}
            ServiceState::Backoff => {
                self.backoff_deadline = None;
                self.transition(ServiceState::Inactive).await;
            }
            ServiceState::Starting | ServiceState::Active => {
                self.begin_stop();
                self.transition(ServiceState::Stopping).await;
            }
        }
    }

    /// `vigil restart` (also `watch-files`): stop, then start again once the
    /// process is gone; start directly if it does not run.
    pub(super) async fn restart(&mut self) -> anyhow::Result<()> {
        if self.state == ServiceState::Stopping {
            // Already stopping: start again once it is gone. Neither signal
            // again nor re-arm the deadline, or repeated restarts would push
            // the SIGKILL further and further out.
            self.pending_restart = true;
        } else if self.state.is_running() {
            self.begin_stop();
            self.transition(ServiceState::Stopping).await;
            self.backoff_deadline = None;
            self.pending_restart = true;
        } else {
            self.try_start().await?;
        }
        Ok(())
    }

    /// Start the process now (a start, restart or scheduled run). A failed
    /// start puts the service into `error` and reports it; `what` names the
    /// occasion in the log and the alert.
    pub(super) async fn start_now(&mut self, what: &str) -> anyhow::Result<()> {
        self.backoff_deadline = None;
        self.transition(ServiceState::Starting).await;
        let res = self.do_start().await;
        if let Err(e) = &res {
            self.start_failed(what, e).await;
        }
        res
    }

    /// Send the stop signal and give the process `kill-delay` before
    /// SIGKILL.
    pub(super) fn begin_stop(&mut self) {
        self.send_stop_signal();
        self.stop_deadline = Some(Instant::now() + self.kill_delay());
    }

    pub(super) async fn handle_child_exit(
        &mut self,
        exit: std::io::Result<std::process::ExitStatus>,
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
        let ready_failed = std::mem::take(&mut self.ready_failed);
        if ready_failed {
            success = false;
            code = None;
        }
        let reason = if ready_failed {
            match self.ready_timeout() {
                Some(t) => format!("did not become ready within {t:?}"),
                None => "did not become ready".to_owned(),
            }
        } else {
            super::health::describe_exit(exit_info)
        };
        self.ready_deadline = None;
        self.notify = None;
        self.stop_watching();
        self.last_exit = exit_info;
        self.metrics
            .set_service_last_exit_code(&self.name, raw_code);

        self.send_event(EventKind::ProcessExited {
            success,
            exit: exit_info,
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
        self.stop_deadline = None;

        if self.state == ServiceState::Stopping {
            self.transition(ServiceState::Inactive).await;
            if std::mem::take(&mut self.pending_restart)
                && self.start_now("restart failed").await.is_ok()
            {
                self.reset_backoff();
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
                self.report_healthy().await;
            }
            Some(OnExit::Ignore) if success && self.is_oneshot() => {
                self.transition(ServiceState::Done).await;
                self.report_healthy().await;
            }
            Some(OnExit::Ignore) => {
                self.transition(ServiceState::Inactive).await;
                if !success {
                    self.report_unhealthy("failed", reason).await;
                }
            }
            Some(
                policy @ (OnExit::Shutdown | OnExit::FailureShutdown | OnExit::SuccessShutdown),
            ) => {
                let exit_code = shutdown_code(self.exit_code_mode, policy, success, code)
                    .expect("shutdown policies always yield a code");
                info!(service = %self.name, ?policy, exit_code, "daemon shutdown requested");
                self.transition(ServiceState::Inactive).await;
                if !success {
                    // Best effort: vigild exits right after this.
                    self.report_unhealthy("failed", format!("{reason}, shutting down vigild"))
                        .await;
                }
                self.send_event(EventKind::DaemonShutdown { exit_code })
                    .await;
            }
            Some(OnExit::Restart) => {
                if self.backoff_limit_exceeded() {
                    error!(service = %self.name, "backoff limit exceeded, giving up");
                    self.transition(ServiceState::Error).await;
                    self.report_unhealthy(
                        "error",
                        format!("{reason}, giving up (backoff limit reached)"),
                    )
                    .await;
                } else {
                    let delay = self.next_backoff();
                    info!(service = %self.name, ?delay, "scheduling restart (backoff)");
                    self.transition(ServiceState::Backoff).await;
                    self.backoff_deadline = Some(Instant::now() + delay);
                    if !success {
                        self.report_unhealthy(
                            "backoff",
                            format!("{reason}, restarting in {delay:?}"),
                        )
                        .await;
                    }
                }
            }
        }
    }

    pub(super) async fn handle_update_config(&mut self, config: vigil_types::plan::ServiceConfig) {
        if self.config == config {
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

    pub(super) async fn handle_backoff_expired(&mut self) {
        // A timer from an earlier backoff that a start or stop made
        // obsolete: the service is not waiting any more. Starting now would
        // replace the running child and leave it unsupervised.
        if self.state != ServiceState::Backoff {
            return;
        }
        info!(service = %self.name, "backoff elapsed, restarting");
        let _ = self.start_now("restart after backoff failed").await;
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
