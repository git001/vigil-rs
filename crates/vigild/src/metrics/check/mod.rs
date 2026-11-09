// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Check metrics: every series has the labels `check` and `level`.
//!
//! The store knows the running checks and their level (`set_checks`, on
//! every (re)plan). Series of a check that was removed, disabled or got
//! another level are dropped then, and writes for a check it does not know
//! (an actor that is shutting down) are ignored, so no stale series
//! survives a replan.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use prometheus_client::metrics::family::Family;
use vigil_types::plan::CheckLevel;

use prometheus_client::metrics::{counter::Counter, gauge::Gauge};
use prometheus_client::registry::Registry;

use super::{MetricsStore, reg, register};

mod labels;

use labels::{
    CheckLabel, DurationFamily, LevelLabel, duration_family, label, level_str, result_label,
};

/// The running checks and their level.
type KnownChecks = std::sync::Mutex<HashMap<String, CheckLevel>>;

pub(super) struct CheckMetrics {
    check_up: Family<CheckLabel, Gauge>,
    check_success_count: Family<CheckLabel, Counter>,
    check_failure_count: Family<CheckLabel, Counter>,
    check_down_count: Family<CheckLabel, Counter>,
    check_down_seconds: Family<CheckLabel, Counter<f64, AtomicU64>>,
    health: Family<LevelLabel, Gauge>,
    check_duration: DurationFamily,
    check_connect_duration: DurationFamily,
    known: KnownChecks,
}

impl CheckMetrics {
    pub(super) fn register(r: &mut Registry) -> Self {
        Self {
            check_up: reg(
                r,
                "vigil_check_up",
                "Whether the health check is up (1) or not (0)",
            ),
            check_success_count: reg(
                r,
                "vigil_check_success_count",
                "Number of times the check has succeeded",
            ),
            check_failure_count: reg(
                r,
                "vigil_check_failure_count",
                "Number of times the check has failed",
            ),
            check_down_count: reg(
                r,
                "vigil_check_down_count",
                "Number of outages: the check went from up to down",
            ),
            check_down_seconds: reg(
                r,
                "vigil_check_down_seconds",
                "Time the check spent down, updated at each run while it is down",
            ),
            health: reg(
                r,
                "vigil_health",
                "Whether GET /v1/health?level= answers healthy (1) or not (0)",
            ),
            check_duration: register(
                r,
                "vigil_check_duration_seconds",
                "Duration of a probe run, from its start to its result (at most the check's timeout)",
                duration_family(),
            ),
            check_connect_duration: register(
                r,
                "vigil_check_connect_duration_seconds",
                "Time until the connection of a probe was up (http: DNS, TCP and TLS; tcp: the connect)",
                duration_family(),
            ),
            known: Default::default(),
        }
    }
}

impl MetricsStore {
    /// The running checks are exactly `checks`: drop the series of every
    /// other check and of a check whose level changed.
    pub fn set_checks(&self, checks: &[(&str, CheckLevel)]) {
        let mut known = self.checks.known.lock().unwrap_or_else(|e| e.into_inner());
        let stale: Vec<(String, CheckLevel)> = known
            .iter()
            .filter(|(name, level)| !checks.contains(&(name.as_str(), **level)))
            .map(|(name, level)| (name.clone(), *level))
            .collect();
        for (name, level) in stale {
            let l = label(&name, level);
            self.checks.check_up.remove(&l);
            self.checks.check_success_count.remove(&l);
            self.checks.check_failure_count.remove(&l);
            self.checks.check_down_count.remove(&l);
            self.checks.check_down_seconds.remove(&l);
            for success in [true, false] {
                let r = result_label(&name, level, success);
                self.checks.check_duration.remove(&r);
                self.checks.check_connect_duration.remove(&r);
            }
        }
        *known = checks
            .iter()
            .map(|(name, level)| ((*name).to_owned(), *level))
            .collect();
    }

    /// The label of `check` if it is a running check with `level`.
    fn known(&self, check: &str, level: CheckLevel) -> Option<CheckLabel> {
        let known = self.checks.known.lock().unwrap_or_else(|e| e.into_inner());
        (known.get(check) == Some(&level)).then(|| label(check, level))
    }

    pub fn record_check_success(&self, check: &str, level: CheckLevel) {
        if let Some(l) = self.known(check, level) {
            self.checks.check_success_count.get_or_create(&l).inc();
        }
    }

    pub fn record_check_failure(&self, check: &str, level: CheckLevel) {
        if let Some(l) = self.known(check, level) {
            self.checks.check_failure_count.get_or_create(&l).inc();
        }
    }

    pub fn set_check_up(&self, check: &str, level: CheckLevel, up: bool) {
        if let Some(l) = self.known(check, level) {
            self.checks.check_up.get_or_create(&l).set(i64::from(up));
        }
    }

    /// The check went from up to down (one outage).
    pub fn record_check_down(&self, check: &str, level: CheckLevel) {
        if let Some(l) = self.known(check, level) {
            self.checks.check_down_count.get_or_create(&l).inc();
        }
    }

    /// Add time the check spent down.
    pub fn add_check_down_seconds(&self, check: &str, level: CheckLevel, seconds: f64) {
        if let Some(l) = self.known(check, level) {
            self.checks
                .check_down_seconds
                .get_or_create(&l)
                .inc_by(seconds);
        }
    }

    /// One probe run: how long it took (`duration`, at most the check's
    /// timeout) and how long its connection took to come up (`connect`,
    /// http and tcp).
    pub fn record_check_run(
        &self,
        check: &str,
        level: CheckLevel,
        success: bool,
        duration: Duration,
        connect: Option<Duration>,
    ) {
        if self.known(check, level).is_none() {
            return;
        }
        let l = result_label(check, level, success);
        self.checks
            .check_duration
            .get_or_create(&l)
            .observe(duration.as_secs_f64());
        if let Some(c) = connect {
            self.checks
                .check_connect_duration
                .get_or_create(&l)
                .observe(c.as_secs_f64());
        }
    }

    /// What `GET /v1/health?level=alive` and `?level=ready` answer now.
    pub fn set_health(&self, alive: bool, ready: bool) {
        for (level, healthy) in [(CheckLevel::Alive, alive), (CheckLevel::Ready, ready)] {
            self.checks
                .health
                .get_or_create(&LevelLabel {
                    level: level_str(level).to_owned(),
                })
                .set(i64::from(healthy));
        }
    }
}
