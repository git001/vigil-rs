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

pub(super) const DEFAULT_PERIOD: Duration = Duration::from_secs(10);
pub(super) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);
pub(super) const DEFAULT_THRESHOLD: u32 = 3;
pub(super) const DEFAULT_CHECK_DELAY: Duration = Duration::from_secs(3);

pub(super) async fn run(
    name: String,
    config: CheckConfig,
    service_configs: Arc<IndexMap<String, ServiceConfig>>,
    mut rx: mpsc::Receiver<Cmd>,
    event_tx: mpsc::Sender<CheckEvent>,
    metrics: Arc<MetricsStore>,
    initial_status: CheckStatus,
) {
    let period = config
        .period
        .as_deref()
        .and_then(|s| parse_duration(s).ok())
        .unwrap_or(DEFAULT_PERIOD);

    let timeout_dur = config
        .timeout
        .as_deref()
        .and_then(|s| parse_duration(s).ok())
        .unwrap_or(DEFAULT_TIMEOUT)
        .min(period);

    let threshold = config.threshold.unwrap_or(DEFAULT_THRESHOLD);

    let http_client = Arc::new(
        HttpClient::builder()
            .timeout(timeout_dur)
            .build()
            .unwrap_or_default(),
    );

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
                    Some(Cmd::GetStatus(reply)) => {
                        let _ = reply.send(CheckInfo {
                            name: name.clone(),
                            level: config.level,
                            status: CheckStatus::Up,
                            failures: 0,
                            threshold: config.threshold.unwrap_or(DEFAULT_THRESHOLD),
                            next_run_in_secs: None,
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
                Some(Cmd::GetStatus(reply)) => {
                    let next = last_tick_at + period;
                    let next_run_in_secs = next
                        .checked_duration_since(tokio::time::Instant::now())
                        .map(|d| d.as_secs());
                    let _ = reply.send(CheckInfo {
                        name: name.clone(),
                        level: config.level,
                        status,
                        failures,
                        threshold,
                        next_run_in_secs,
                    });
                }
            },

            _ = tick.tick() => {
                last_tick_at = tokio::time::Instant::now();
                let ok = perform(&config, timeout_dur, &http_client, &service_configs).await;
                if ok {
                    metrics.record_check_success(&name);
                    if status == CheckStatus::Down {
                        info!(check = %name, "check recovered");
                        status = CheckStatus::Up;
                        metrics.set_check_up(&name, true);
                        let _ = event_tx.send(CheckEvent { check: name.clone(), status }).await;
                    }
                    failures = 0;
                } else {
                    metrics.record_check_failure(&name);
                    failures += 1;
                    warn!(check = %name, failures, threshold, "check failed");
                    if failures >= threshold && status == CheckStatus::Up {
                        info!(check = %name, "check is down");
                        status = CheckStatus::Down;
                        metrics.set_check_up(&name, false);
                        let _ = event_tx.send(CheckEvent { check: name.clone(), status }).await;
                    }
                }
                // On the first run, always report current status so AlertSender
                // knows the initial state (e.g. a healthy check shows "up" instead
                // of "unknown" in `vigil alerts list`).
                if std::mem::replace(&mut first_run, false) {
                    let _ = event_tx.send(CheckEvent { check: name.clone(), status }).await;
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
