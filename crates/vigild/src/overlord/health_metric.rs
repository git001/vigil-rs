// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Check metrics the overlord keeps: which checks run (and so which series
//! exist) and `vigil_health{level}`.

use vigil_types::api::CheckStatus;
use vigil_types::plan::{CheckLevel, Startup};

use super::Overlord;

impl Overlord {
    /// Tell the metrics store which checks run (not `startup: disabled`),
    /// with their level. Before the check actors are (re)spawned.
    pub(super) fn publish_running_checks(&self) {
        let running: Vec<(&str, CheckLevel)> = self
            .plan
            .checks
            .iter()
            .filter(|(_, c)| c.startup() != Startup::Disabled)
            .map(|(name, c)| (name.as_str(), c.level()))
            .collect();
        self.metrics.set_checks(&running);
    }

    /// Set `vigil_health{level}` to what `GET /v1/health?level=` answers:
    /// `alive` is healthy while no running alive check is down, `ready`
    /// while no running check is down. A check that has not run yet counts
    /// as up, as in `GET /v1/health`.
    pub(super) fn publish_health(&self) {
        let mut alive = true;
        let mut ready = true;
        for (name, config) in &self.plan.checks {
            if config.startup() == Startup::Disabled {
                continue;
            }
            if self.alert_sender.check_status(name) == Some(CheckStatus::Down) {
                ready = false;
                if config.level() == CheckLevel::Alive {
                    alive = false;
                }
            }
        }
        self.metrics.set_health(alive, ready);
    }
}
