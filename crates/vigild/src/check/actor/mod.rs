// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Check actor loop.

use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use reqwest::Client as HttpClient;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, info, warn};
use vigil_types::api::{CheckInfo, CheckRefresh, CheckStatus};
use vigil_types::plan::{CheckConfig, CheckLevel, ServiceConfig};

use crate::duration::parse_duration;
use crate::metrics::MetricsStore;

use super::probe::{ProbeResult, perform};
use super::{CheckEvent, Cmd};

/// A check's config and the HTTP client built for it when loading.
pub(super) struct Setup {
    pub config: CheckConfig,
    pub http: Option<HttpClient>,
}

pub(super) const DEFAULT_PERIOD: Duration = Duration::from_secs(10);
pub(super) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);
pub(super) const DEFAULT_THRESHOLD: u32 = 3;
pub(super) const DEFAULT_CHECK_DELAY: Duration = Duration::from_secs(3);

/// The check's `period`. The plan check enforces 1s..24h; never let a
/// zero period reach `interval()` (it panics) or an unbounded one through.
/// Sub-second periods stay possible for unit tests, which bypass the plan
/// check.
fn period_of(config: &CheckConfig) -> Duration {
    let period = config
        .period
        .as_deref()
        .and_then(|s| parse_duration(s).ok())
        .unwrap_or(DEFAULT_PERIOD);
    if period.is_zero() {
        DEFAULT_PERIOD
    } else {
        period.min(crate::duration::MAX_INTERVAL)
    }
}

/// The check's `timeout`, at most its period.
pub(super) fn timeout_of(config: &CheckConfig) -> Duration {
    config
        .timeout
        .as_deref()
        .and_then(|s| parse_duration(s).ok())
        .unwrap_or(DEFAULT_TIMEOUT)
        .min(period_of(config))
}

/// One check's state and what it needs to run a probe.
struct Checker {
    name: String,
    config: CheckConfig,
    http: Option<HttpClient>,
    service_configs: Arc<IndexMap<String, ServiceConfig>>,
    event_tx: mpsc::Sender<CheckEvent>,
    metrics: Arc<MetricsStore>,
    timeout_dur: Duration,
    threshold: u32,
    failures: u32,
    status: CheckStatus,
    first_run: bool,
    /// Why the last run failed; cleared by a pass.
    last_error: Option<String>,
    /// Since when the down time was last counted
    /// (`vigil_check_down_seconds`); `None` while up.
    down_since: Option<std::time::Instant>,
}

impl Checker {
    /// Run the probe once and count the result: failures, threshold,
    /// status, events (and so alerts). A scheduled run and a refresh
    /// (`POST /v1/checks/refresh`) both go through here.
    async fn run_once(&mut self) -> ProbeResult {
        let name = self.name.clone();
        let level = self.config.level();
        let probe_started = std::time::Instant::now();
        let result = perform(
            &self.config,
            self.timeout_dur,
            self.http.as_ref(),
            &self.service_configs,
        )
        .await;
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
    fn count_down_time(&mut self, level: CheckLevel) {
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

    fn info(&self, next_run_in_secs: Option<u64>) -> CheckInfo {
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
    async fn refresh(&mut self, reply: oneshot::Sender<CheckRefresh>, next: Option<u64>) {
        let error = self.run_once().await.err();
        let _ = reply.send(CheckRefresh {
            info: self.info(next),
            error,
        });
    }
}

pub(super) async fn run(
    name: String,
    setup: Setup,
    service_configs: Arc<IndexMap<String, ServiceConfig>>,
    mut rx: mpsc::Receiver<Cmd>,
    event_tx: mpsc::Sender<CheckEvent>,
    metrics: Arc<MetricsStore>,
    initial_status: CheckStatus,
) {
    let Setup { config, http } = setup;
    let period = period_of(&config);
    let timeout_dur = timeout_of(&config);
    let threshold = config.threshold.unwrap_or(DEFAULT_THRESHOLD);
    let delay_dur = config
        .delay
        .as_deref()
        .and_then(|s| parse_duration(s).ok())
        .unwrap_or(DEFAULT_CHECK_DELAY);

    let mut checker = Checker {
        name,
        config,
        http,
        service_configs,
        event_tx,
        metrics,
        timeout_dur,
        threshold,
        failures: 0,
        status: initial_status,
        first_run: true,
        last_error: None,
        // A check restarted by a replan may start down (initial_status).
        down_since: (initial_status == CheckStatus::Down).then(std::time::Instant::now),
    };
    let level = checker.config.level();
    checker
        .metrics
        .set_check_up(&checker.name, level, checker.status == CheckStatus::Up);

    // Wait for the initial delay before the first scheduled run (default:
    // 3s). Answers GetStatus, Refresh and Shutdown meanwhile.
    {
        let deadline = tokio::time::Instant::now() + delay_dur;
        loop {
            tokio::select! {
                biased;
                cmd = rx.recv() => match cmd {
                    None | Some(Cmd::Shutdown) => {
                        checker.count_down_time(level);
                        return;
                    }
                    Some(Cmd::SetHttpClient(c)) => checker.http = Some(c),
                    Some(Cmd::GetStatus(reply)) => {
                        let _ = reply.send(checker.info(None));
                    }
                    Some(Cmd::Refresh(reply)) => checker.refresh(reply, None).await,
                },
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
    }

    let mut tick = interval(period);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Tracks when the last tick fired so we can report time-until-next-run.
    let mut last_tick_at = tokio::time::Instant::now();
    let next_run = |last_tick_at: tokio::time::Instant| {
        (last_tick_at + period)
            .checked_duration_since(tokio::time::Instant::now())
            .map(|d| d.as_secs())
    };

    loop {
        tokio::select! {
            biased;

            cmd = rx.recv() => match cmd {
                None | Some(Cmd::Shutdown) => break,
                Some(Cmd::SetHttpClient(c)) => checker.http = Some(c),
                Some(Cmd::GetStatus(reply)) => {
                    let _ = reply.send(checker.info(next_run(last_tick_at)));
                }
                Some(Cmd::Refresh(reply)) => {
                    checker.refresh(reply, next_run(last_tick_at)).await;
                }
            },

            _ = tick.tick() => {
                last_tick_at = tokio::time::Instant::now();
                let _ = checker.run_once().await;
            }
        }
    }

    checker.count_down_time(level);
    debug!(check = %checker.name, "check actor shut down");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
