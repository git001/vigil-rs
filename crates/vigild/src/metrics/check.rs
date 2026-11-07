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

use prometheus_client::encoding::EncodeLabelSet;
use vigil_types::plan::CheckLevel;

use super::MetricsStore;

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(super) struct CheckLabel {
    check: String,
    /// `alive` or `ready`.
    level: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(super) struct LevelLabel {
    /// `alive` or `ready`.
    level: String,
}

fn level_str(level: CheckLevel) -> &'static str {
    match level {
        CheckLevel::Alive => "alive",
        CheckLevel::Ready => "ready",
    }
}

fn label(check: &str, level: CheckLevel) -> CheckLabel {
    CheckLabel {
        check: check.to_owned(),
        level: level_str(level).to_owned(),
    }
}

/// The running checks and their level.
pub(super) type KnownChecks = std::sync::Mutex<HashMap<String, CheckLevel>>;

impl MetricsStore {
    /// The running checks are exactly `checks`: drop the series of every
    /// other check and of a check whose level changed.
    pub fn set_checks(&self, checks: &[(&str, CheckLevel)]) {
        let mut known = self.known_checks.lock().unwrap_or_else(|e| e.into_inner());
        let stale: Vec<(String, CheckLevel)> = known
            .iter()
            .filter(|(name, level)| !checks.contains(&(name.as_str(), **level)))
            .map(|(name, level)| (name.clone(), *level))
            .collect();
        for (name, level) in stale {
            let l = label(&name, level);
            self.check_up.remove(&l);
            self.check_success_count.remove(&l);
            self.check_failure_count.remove(&l);
            self.check_down_count.remove(&l);
            self.check_down_seconds.remove(&l);
        }
        *known = checks
            .iter()
            .map(|(name, level)| ((*name).to_owned(), *level))
            .collect();
    }

    /// The label of `check` if it is a running check with `level`.
    fn known(&self, check: &str, level: CheckLevel) -> Option<CheckLabel> {
        let known = self.known_checks.lock().unwrap_or_else(|e| e.into_inner());
        (known.get(check) == Some(&level)).then(|| label(check, level))
    }

    pub fn record_check_success(&self, check: &str, level: CheckLevel) {
        if let Some(l) = self.known(check, level) {
            self.check_success_count.get_or_create(&l).inc();
        }
    }

    pub fn record_check_failure(&self, check: &str, level: CheckLevel) {
        if let Some(l) = self.known(check, level) {
            self.check_failure_count.get_or_create(&l).inc();
        }
    }

    pub fn set_check_up(&self, check: &str, level: CheckLevel, up: bool) {
        if let Some(l) = self.known(check, level) {
            self.check_up.get_or_create(&l).set(i64::from(up));
        }
    }

    /// The check went from up to down (one outage).
    pub fn record_check_down(&self, check: &str, level: CheckLevel) {
        if let Some(l) = self.known(check, level) {
            self.check_down_count.get_or_create(&l).inc();
        }
    }

    /// Add time the check spent down.
    pub fn add_check_down_seconds(&self, check: &str, level: CheckLevel, seconds: f64) {
        if let Some(l) = self.known(check, level) {
            self.check_down_seconds.get_or_create(&l).inc_by(seconds);
        }
    }

    /// What `GET /v1/health?level=alive` and `?level=ready` answer now.
    pub fn set_health(&self, alive: bool, ready: bool) {
        for (level, healthy) in [(CheckLevel::Alive, alive), (CheckLevel::Ready, ready)] {
            self.health
                .get_or_create(&LevelLabel {
                    level: level_str(level).to_owned(),
                })
                .set(i64::from(healthy));
        }
    }
}
