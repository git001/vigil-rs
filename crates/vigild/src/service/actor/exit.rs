// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The process ended: classify the exit and apply the exit policy.

use tokio::time::Instant;
use tracing::{error, info};
use vigil_types::plan::OnExit;

use crate::state::ServiceState;

use super::super::{effective_on_failure, effective_on_success, shutdown_code};
use super::{Actor, EventKind};

impl Actor {
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
        self.apply_exit_policy(success, code, reason).await;
    }

    /// What `on-success` / `on-failure` say after the process ended on its
    /// own (not stopped by vigild): done, inactive, shutdown or backoff.
    async fn apply_exit_policy(&mut self, success: bool, code: Option<i32>, reason: String) {
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
}
