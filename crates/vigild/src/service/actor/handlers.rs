// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Command handlers — Start, Stop, Restart, config update, kill deadline,
//! backoff, cleanup.

use std::time::Duration;

use tokio::time::Instant;
use tracing::{info, warn};

use crate::state::ServiceState;

use super::Actor;

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
