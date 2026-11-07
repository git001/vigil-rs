// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! One check's state: run the probe, count failures, report status.

use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};
use vigil_types::api::{CheckInfo, CheckRefresh, CheckStatus};
use vigil_types::plan::{CheckConfig, CheckLevel, ServiceConfig};

use crate::metrics::MetricsStore;

use super::super::CheckEvent;
use super::super::probe::{CheckClient, ProbeResult, perform};

/// One check's state and what it needs to run a probe.
pub(super) struct Checker {
    pub(super) name: String,
    pub(super) config: CheckConfig,
    pub(super) http: Option<CheckClient>,
    pub(super) service_configs: Arc<IndexMap<String, ServiceConfig>>,
    pub(super) event_tx: mpsc::Sender<CheckEvent>,
    pub(super) metrics: Arc<MetricsStore>,
    pub(super) timeout_dur: Duration,
    pub(super) threshold: u32,
    pub(super) failures: u32,
    pub(super) status: CheckStatus,
    pub(super) first_run: bool,
    /// Why the last run failed; cleared by a pass.
    pub(super) last_error: Option<String>,
    /// Since when the down time was last counted
    /// (`vigil_check_down_seconds`); `None` while up.
    pub(super) down_since: Option<std::time::Instant>,
}

impl Checker {
    /// Run the probe once and count the result: failures, threshold,
    /// status, events (and so alerts). A scheduled run and a refresh
    /// (`POST /v1/checks/refresh`) both go through here.
    pub(super) async fn run_once(&mut self) -> ProbeResult {
        let name = self.name.clone();
        let level = self.config.level();
        let probe_started = std::time::Instant::now();
        let probe = perform(
            &self.config,
            self.timeout_dur,
            self.http.as_ref(),
            &self.service_configs,
        )
        .await;
        // A probe that ran out of time took its timeout; the exec output
        // drain after a kill does not count.
        let duration = probe_started.elapsed().min(self.timeout_dur);
        let result = probe.result;
        self.metrics
            .record_check_run(&name, level, result.is_ok(), duration, probe.connect);
        let first = std::mem::replace(&mut self.first_run, false);
        match &result {
            Err(reason) => {
                self.metrics.record_check_failure(&name, level);
                self.failures += 1;
                let (failures, threshold) = (self.failures, self.threshold);
                warn!(check = %name, failures, threshold, error = ?reason, "check failed");
                let down = failures >= threshold && self.status == CheckStatus::Up;
                if down {
                    info!(check = %name, error = ?reason, "check is down");
                    self.status = CheckStatus::Down;
                    self.metrics.set_check_up(&name, level, false);
                    self.metrics.record_check_down(&name, level);
                    self.down_since = Some(std::time::Instant::now());
                }
                self.count_down_time(level);
                let error = (self.status == CheckStatus::Down).then(|| reason.clone());
                self.last_error = Some(reason.clone());
                // A Down transition is reported; on the first run the
                // current status is reported anyway, so AlertSender knows
                // the initial state.
                if down || first {
                    self.send(None, error).await;
                }
            }
            Ok(()) => {
                self.last_error = None;
                self.metrics.record_check_success(&name, level);
                if self.status == CheckStatus::Down {
                    info!(check = %name, "check recovered");
                    self.count_down_time(level);
                    self.down_since = None;
                    self.status = CheckStatus::Up;
                    self.metrics.set_check_up(&name, level, true);
                }
                self.failures = 0;
                // Every pass is reported (for `ready: check`); it also
                // covers recovery and the initial status. Alerts only fire
                // on status changes.
                self.send(Some(probe_started), None).await;
            }
        }
        result
    }

    /// Add the time since the last count to `vigil_check_down_seconds`.
    pub(super) fn count_down_time(&mut self, level: CheckLevel) {
        if let Some(since) = self.down_since.as_mut() {
            let now = std::time::Instant::now();
            let seconds = now.duration_since(*since).as_secs_f64();
            self.metrics
                .add_check_down_seconds(&self.name, level, seconds);
            *since = now;
        }
    }

    async fn send(&self, passed_at: Option<std::time::Instant>, error: Option<String>) {
        let _ = self
            .event_tx
            .send(CheckEvent {
                check: self.name.clone(),
                status: self.status,
                passed_at,
                error,
            })
            .await;
    }

    pub(super) fn info(&self, next_run_in_secs: Option<u64>) -> CheckInfo {
        CheckInfo {
            name: self.name.clone(),
            level: self.config.level(),
            status: self.status,
            failures: self.failures,
            threshold: self.threshold,
            next_run_in_secs,
            last_error: self.last_error.clone(),
        }
    }

    /// Answer a refresh: run now, reply with the state after the run.
    pub(super) async fn refresh(
        &mut self,
        reply: oneshot::Sender<CheckRefresh>,
        next: Option<u64>,
    ) {
        let error = self.run_once().await.err();
        let _ = reply.send(CheckRefresh {
            info: self.info(next),
            error,
        });
    }
}
