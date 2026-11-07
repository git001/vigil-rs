// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Check results: alerts (quiet while a service of the check is not
//! healthy), `ready: check` and `on-check-failure`.

use tracing::{error, info};
use vigil_types::api::CheckStatus;
use vigil_types::plan::OnExit;

use crate::alert::AlertEvent;
use crate::check::CheckEvent;
use crate::service::{self, Cmd as SvcCmd};
use crate::state::ServiceState;

use super::Overlord;

impl Overlord {
    pub(super) async fn handle_check_event(&mut self, event: CheckEvent) {
        if let Some(started_at) = event.passed_at {
            self.forward_check_passed(&event.check, started_at).await;
        }
        if self.check_quiet(&event.check) {
            self.alert_sender
                .hold_check_event(&event.check, event.status, event.error.as_deref());
        } else {
            let fired = self.alert_sender.handle_check_event(
                &event.check,
                event.status,
                event.error.as_deref(),
            );
            self.count_fired(&fired);
        }
        self.publish_health();

        if event.status != CheckStatus::Down {
            return;
        }
        if event.passed_at.is_some() {
            return; // a pass is never a Down transition
        }

        let actions: Vec<(String, OnExit)> = self
            .plan
            .services
            .iter()
            .filter_map(|(name, cfg)| {
                cfg.on_check_failure
                    .get(&event.check)
                    .map(|a| (name.clone(), *a))
            })
            .collect();

        for (svc_name, action) in actions {
            match action {
                OnExit::Restart => {
                    info!(service = %svc_name, check = %event.check, "on-check-failure: restarting service");
                    if let Err(e) = self.svc_cmd(&svc_name, SvcCmd::Restart).await {
                        error!(service = %svc_name, error = %e, "on-check-failure restart failed");
                    }
                }
                OnExit::Ignore => {}
                policy @ (OnExit::Shutdown | OnExit::FailureShutdown | OnExit::SuccessShutdown) => {
                    // No process exit code here: a check failure is a failure.
                    let code = service::shutdown_code(self.exit_code_mode, policy, false, None)
                        .expect("shutdown policies always yield a code");
                    info!(service = %svc_name, check = %event.check, ?policy, "on-check-failure: shutdown");
                    self.shutdown(code).await;
                }
            }
        }
    }

    /// Send an alert event of any component (check, service, vigild).
    pub(super) fn send_alert(&mut self, ev: AlertEvent) {
        let fired = self.alert_sender.send(ev);
        self.count_fired(&fired);
    }

    pub(super) fn count_fired(&self, fired: &[String]) {
        for alert_name in fired {
            self.metrics.record_alert_fire(alert_name);
        }
    }

    /// Services `check` belongs to: they name it in `ready-check` or
    /// `on-check-failure`, or the check names them in `service`.
    fn services_of_check<'a>(&'a self, check: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        let named = self
            .plan
            .checks
            .get(check)
            .and_then(|c| c.service.as_deref())
            .filter(|s| self.plan.services.contains_key(*s));
        self.plan
            .services
            .iter()
            .filter(move |(name, svc)| {
                svc.ready_check.as_deref() == Some(check)
                    || svc.on_check_failure.contains_key(check)
                    || named == Some(name.as_str())
            })
            .map(|(name, _)| name.as_str())
    }

    /// Checks that belong to `service` (see [`Self::services_of_check`]).
    pub(super) fn checks_of_service(&self, service: &str) -> Vec<String> {
        self.plan
            .checks
            .keys()
            .filter(|c| self.services_of_check(c).any(|s| s == service))
            .cloned()
            .collect()
    }

    /// A check sends no alerts while a service it belongs to is not
    /// healthy: the service alert already says what is wrong.
    pub(super) fn check_quiet(&self, check: &str) -> bool {
        self.services_of_check(check)
            .any(|s| !self.healthy.contains(s))
    }

    /// Tell services that wait for `check` (`ready: check`) about a pass.
    async fn forward_check_passed(&self, check: &str, started_at: std::time::Instant) {
        for (name, cfg) in &self.plan.services {
            if cfg.ready != Some(vigil_types::plan::ReadyMode::Check)
                || cfg.ready_check.as_deref() != Some(check)
            {
                continue;
            }
            let Some(entry) = self.services.get(name) else {
                continue;
            };
            if entry.snapshot.state != ServiceState::Starting {
                continue;
            }
            let _ = entry
                .handle
                .tx
                .send(SvcCmd::CheckPassed {
                    check: check.to_string(),
                    started_at,
                })
                .await;
        }
    }
}
