// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Check actor loop.

use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use reqwest::Client as HttpClient;
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, info, warn};
use vigil_types::api::{CheckInfo, CheckStatus};
use vigil_types::plan::{CheckConfig, ServiceConfig};

use crate::duration::parse_duration;
use crate::metrics::MetricsStore;

use super::probe::perform;
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

pub(super) async fn run(
    name: String,
    setup: Setup,
    service_configs: Arc<IndexMap<String, ServiceConfig>>,
    mut rx: mpsc::Receiver<Cmd>,
    event_tx: mpsc::Sender<CheckEvent>,
    metrics: Arc<MetricsStore>,
    initial_status: CheckStatus,
) {
    let Setup { config, mut http } = setup;
    let period = config
        .period
        .as_deref()
        .and_then(|s| parse_duration(s).ok())
        .unwrap_or(DEFAULT_PERIOD);
    // The plan check enforces 1s..24h; never let a zero period reach
    // `interval()` (it panics) or an unbounded one through. Sub-second
    // periods stay possible for unit tests, which bypass the plan check.
    let period = if period.is_zero() {
        DEFAULT_PERIOD
    } else {
        period.min(crate::duration::MAX_INTERVAL)
    };

    let timeout_dur = config
        .timeout
        .as_deref()
        .and_then(|s| parse_duration(s).ok())
        .unwrap_or(DEFAULT_TIMEOUT)
        .min(period);

    let threshold = config.threshold.unwrap_or(DEFAULT_THRESHOLD);

    // Wait for the initial delay before the first check (default: 3s).
    // Responds to GetStatus (reports "up, 0 failures") and Shutdown during the wait.
    let delay_dur = config
        .delay
        .as_deref()
        .and_then(|s| parse_duration(s).ok())
        .unwrap_or(DEFAULT_CHECK_DELAY);
    {
        let deadline = tokio::time::Instant::now() + delay_dur;
        loop {
            tokio::select! {
                biased;
                cmd = rx.recv() => match cmd {
                    None | Some(Cmd::Shutdown) => return,
                    Some(Cmd::SetHttpClient(c)) => http = Some(c),
                    Some(Cmd::GetStatus(reply)) => {
                        let _ = reply.send(CheckInfo {
                            name: name.clone(),
                            level: config.level(),
                            status: CheckStatus::Up,
                            failures: 0,
                            threshold: config.threshold.unwrap_or(DEFAULT_THRESHOLD),
                            next_run_in_secs: None,
                            last_error: None,
                        });
                    }
                },
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
    }

    let mut failures: u32 = 0;
    let mut status = initial_status;
    let mut first_run = true;
    let mut last_error: Option<String> = None;
    metrics.set_check_up(&name, status == CheckStatus::Up);

    let mut tick = interval(period);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Tracks when the last tick fired so we can report time-until-next-run.
    let mut last_tick_at = tokio::time::Instant::now();

    loop {
        tokio::select! {
            biased;

            cmd = rx.recv() => match cmd {
                None | Some(Cmd::Shutdown) => break,
                Some(Cmd::SetHttpClient(c)) => http = Some(c),
                Some(Cmd::GetStatus(reply)) => {
                    let next = last_tick_at + period;
                    let next_run_in_secs = next
                        .checked_duration_since(tokio::time::Instant::now())
                        .map(|d| d.as_secs());
                    let _ = reply.send(CheckInfo {
                        name: name.clone(),
                        level: config.level(),
                        status,
                        failures,
                        threshold,
                        next_run_in_secs,
                        last_error: last_error.clone(),
                    });
                }
            },

            _ = tick.tick() => {
                last_tick_at = tokio::time::Instant::now();
                let probe_started = std::time::Instant::now();
                let result = perform(&config, timeout_dur, http.as_ref(), &service_configs).await;
                let first = std::mem::replace(&mut first_run, false);
                if let Err(reason) = result {
                    metrics.record_check_failure(&name);
                    failures += 1;
                    warn!(check = %name, failures, threshold, error = ?reason, "check failed");
                    let down = failures >= threshold && status == CheckStatus::Up;
                    if down {
                        info!(check = %name, error = ?reason, "check is down");
                        status = CheckStatus::Down;
                        metrics.set_check_up(&name, false);
                    }
                    let error = (status == CheckStatus::Down).then(|| reason.clone());
                    last_error = Some(reason);
                    // A Down transition is reported; on the first run the
                    // current status is reported anyway, so AlertSender
                    // knows the initial state.
                    if down || first {
                        let _ = event_tx
                            .send(CheckEvent { check: name.clone(), status, passed_at: None, error })
                            .await;
                    }
                } else {
                    last_error = None;
                    metrics.record_check_success(&name);
                    if status == CheckStatus::Down {
                        info!(check = %name, "check recovered");
                        status = CheckStatus::Up;
                        metrics.set_check_up(&name, true);
                    }
                    failures = 0;
                    // Every pass is reported (for `ready: check`); it also
                    // covers recovery and the initial status. Alerts only
                    // fire on status changes.
                    let _ = event_tx
                        .send(CheckEvent {
                            check: name.clone(),
                            status,
                            passed_at: Some(probe_started),
                            error: None,
                        })
                        .await;
                }
            }
        }
    }

    debug!(check = %name, "check actor shut down");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
