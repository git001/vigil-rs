// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Check actor loop.

use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval_at};
use tracing::debug;
use vigil_types::api::CheckStatus;
use vigil_types::plan::{CheckConfig, ServiceConfig};

use crate::duration::parse_duration;
use crate::metrics::MetricsStore;

use super::probe::CheckClient;
use super::{CheckEvent, Cmd};

mod checker;
use checker::Checker;

/// A check's config and the HTTP client built for it when loading.
pub(super) struct Setup {
    pub config: CheckConfig,
    pub http: Option<CheckClient>,
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

    // The first scheduled run comes after the initial delay (default 3s),
    // then one every period. Before it, GetStatus and Refresh report no next
    // run.
    let mut tick = interval_at(tokio::time::Instant::now() + delay_dur, period);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // When the last tick fired, to report the time until the next run.
    let mut last_tick_at: Option<tokio::time::Instant> = None;
    let next_run = |last_tick_at: Option<tokio::time::Instant>| {
        (last_tick_at? + period)
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
                last_tick_at = Some(tokio::time::Instant::now());
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
