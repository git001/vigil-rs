// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Read-only queries: services, checks, health and alerts.

use tokio::sync::oneshot;
use vigil_types::api::{AlertInfo, CheckStatus, ServiceInfo};

use crate::check;

use super::Overlord;

impl Overlord {
    pub(super) fn get_service_infos(&self, names: &[String]) -> Vec<ServiceInfo> {
        self.services
            .values()
            .filter(|e| names.is_empty() || names.contains(&e.snapshot.name))
            .map(|e| {
                let cfg = self.plan.services.get(&e.snapshot.name);
                let stop_signal = cfg
                    .and_then(|c| c.stop_signal)
                    .map(|s| format!("{:?}", s.0))
                    .unwrap_or_else(|| "SIGTERM".into());
                let on_success = match cfg.map(crate::service::effective_on_success) {
                    Some(Some(p)) => p.as_str(),
                    Some(None) => "done", // oneshot
                    None => "restart",
                };
                let on_failure = cfg.map_or("restart", |c| {
                    crate::service::effective_on_failure(c).as_str()
                });
                ServiceInfo {
                    name: e.snapshot.name.clone(),
                    startup: e.snapshot.startup,
                    current: e.snapshot.state.to_api_status(),
                    current_since: Some(e.snapshot.since),
                    stop_signal,
                    on_success: on_success.into(),
                    on_failure: on_failure.into(),
                    last_exit: e.snapshot.last_exit,
                    status_message: e.snapshot.status_message.clone(),
                    next_run: e.snapshot.next_run,
                    waiting_for: if self.pending_autostart.contains(&e.snapshot.name) {
                        self.unmet_deps(&e.snapshot.name)
                    } else {
                        Vec::new()
                    },
                }
            })
            .collect()
    }

    /// The checks of the plan among `names` (empty: all), in plan order. A
    /// check with `startup: disabled` has no actor and is `inactive`.
    pub(super) async fn query_checks(&self, names: &[String]) -> Vec<vigil_types::api::CheckInfo> {
        let mut infos = Vec::new();
        for (name, config) in &self.plan.checks {
            if !names.is_empty() && !names.contains(name) {
                continue;
            }
            let Some(entry) = self.checks.get(name) else {
                infos.push(check::inactive_info(name.clone(), config));
                continue;
            };
            let (tx, rx) = oneshot::channel();
            if entry
                .handle
                .tx
                .send(check::Cmd::GetStatus(tx))
                .await
                .is_ok()
                && let Ok(info) = rx.await
            {
                infos.push(info);
            }
        }
        infos
    }

    /// `GET /v1/health`: the checks of `level` (`ready` includes
    /// `alive` checks; none: all levels) among `names` (empty: all) are
    /// healthy when none of them is down. Only running checks count; a
    /// check with `startup: disabled` is ignored. A name that is not a check
    /// is an error, so a typo in a probe cannot pass for ever.
    pub(super) async fn health(
        &self,
        level: Option<vigil_types::plan::CheckLevel>,
        names: &[String],
    ) -> Result<bool, String> {
        use vigil_types::plan::CheckLevel;
        if let Some(unknown) = names.iter().find(|n| !self.plan.checks.contains_key(*n)) {
            return Err(format!("check {unknown:?} not found"));
        }
        let counts = |l: CheckLevel| match level {
            None | Some(CheckLevel::Ready) => true,
            Some(CheckLevel::Alive) => l == CheckLevel::Alive,
        };
        Ok(!self
            .query_checks(names)
            .await
            .iter()
            .any(|c| counts(c.level) && c.status == CheckStatus::Down))
    }

    pub(super) fn query_alerts(&self, names: &[String]) -> Vec<AlertInfo> {
        self.plan
            .alerts
            .iter()
            .filter(|(name, _)| names.is_empty() || names.contains(name))
            .map(|(name, cfg)| {
                let check_status: Vec<vigil_types::api::AlertCheckStatus> = cfg
                    .on_check
                    .iter()
                    .map(|c| vigil_types::api::AlertCheckStatus {
                        check: c.clone(),
                        status: self.alert_sender.check_status(c),
                    })
                    .collect();
                AlertInfo {
                    name: name.clone(),
                    url: cfg.url.clone(),
                    format: cfg.format(),
                    on_check: cfg.on_check.clone(),
                    on_service: cfg.on_service.clone(),
                    on_vigild: cfg.on_vigild.clone(),
                    check_status,
                }
            })
            .collect()
    }
}
