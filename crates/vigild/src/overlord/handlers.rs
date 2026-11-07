// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Overlord command dispatch: handles `Cmd` messages from the HTTP API.

use chrono::Utc;
use tokio::sync::oneshot;
use vigil_types::api::{AlertInfo, ChangeInfo, ChangeStatus, CheckStatus, ServiceInfo, SystemInfo};
use vigil_types::plan::OnExit;

use crate::check;
use crate::service::Cmd as SvcCmd;

use super::{Cmd, Overlord};

impl Overlord {
    pub(super) async fn handle_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Services {
                action,
                names,
                reply,
            } => {
                let _ = reply.send(self.handle_services_action(action, names).await);
            }
            Cmd::GetServices { names, reply } => {
                let _ = reply.send(self.get_service_infos(&names));
            }
            Cmd::GetChanges { id, reply } => {
                let result = match id {
                    None => self.changes.clone(),
                    Some(ref id) => self
                        .changes
                        .iter()
                        .filter(|c| &c.id == id)
                        .cloned()
                        .collect(),
                };
                let _ = reply.send(result);
            }
            Cmd::GetChecks { names, reply } => {
                let infos = self.query_checks(&names).await;
                let _ = reply.send(infos);
            }
            Cmd::RefreshCheck { name, reply } => self.refresh_check(name, reply),
            Cmd::Health {
                level,
                names,
                reply,
            } => {
                let _ = reply.send(self.health(level, &names).await);
            }
            Cmd::GetAlerts { names, reply } => {
                let infos = self.query_alerts(&names);
                let _ = reply.send(infos);
            }
            Cmd::GetSystemInfo { reply } => {
                let _ = reply.send(SystemInfo {
                    boot_id: self.boot_id.clone(),
                    start_time: self.start_time,
                    http_address: self.http_address.clone(),
                    https_address: self.https_address.clone(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                });
            }
            Cmd::ReloadLayers { reply } => {
                let _ = reply.send(self.reload_layers().await);
            }
            Cmd::ForwardSignal { signal } => {
                self.forward_signal(signal).await;
            }
            Cmd::Shutdown => unreachable!(),
        }
    }

    async fn handle_services_action(
        &mut self,
        action: vigil_types::api::ServiceAction,
        names: Vec<String>,
    ) -> anyhow::Result<ChangeInfo> {
        use vigil_types::api::ServiceAction;

        if action == ServiceAction::Autostart {
            let (targets, errors) = self.autostart_action(names).await;
            return Ok(self.record_change(action, &targets, errors));
        }

        let targets: Vec<String> = if names.is_empty() {
            self.plan.services.keys().cloned().collect()
        } else {
            names
        };

        let mut errors: Vec<String> = Vec::new();
        for name in &targets {
            let res = match action {
                ServiceAction::Start => self.svc_cmd(name, SvcCmd::Start).await,
                ServiceAction::Stop => self.svc_cmd(name, SvcCmd::Stop).await,
                ServiceAction::Restart => self.svc_cmd(name, SvcCmd::Restart).await,
                ServiceAction::Replan => self.reload_layers().await,
                ServiceAction::Autostart => unreachable!("handled above"),
            };
            if let Err(e) = res {
                errors.push(format!("{name}: {e:#}"));
            }
        }

        Ok(self.record_change(action, &targets, errors))
    }

    /// `autostart`: the services the startup autostart covers (`startup:
    /// enabled`, not scheduled, no finished oneshot), each started once its
    /// `after` dependencies are ready. Without names all of them; with names
    /// only those, and each name autostart does not cover is an error. The
    /// plan is not changed.
    async fn autostart_action(&mut self, names: Vec<String>) -> (Vec<String>, Vec<String>) {
        let (targets, mut errors) = if names.is_empty() {
            (self.autostart_names(), Vec::new())
        } else {
            let mut targets = Vec::new();
            let mut errors = Vec::new();
            for name in names {
                match self.autostart_skip_reason(&name) {
                    None => targets.push(name),
                    Some(why) => errors.push(format!("{name}: {why}")),
                }
            }
            (targets, errors)
        };
        errors.extend(self.start_or_defer(targets.clone()).await);
        (targets, errors)
    }

    /// Record a finished change (all service actions are synchronous).
    fn record_change(
        &mut self,
        action: vigil_types::api::ServiceAction,
        targets: &[String],
        errors: Vec<String>,
    ) -> ChangeInfo {
        self.change_counter += 1;
        let now = Utc::now();
        let (status, err_msg) = if errors.is_empty() {
            (ChangeStatus::Done, None)
        } else {
            (ChangeStatus::Error, Some(errors.join("; ")))
        };
        let change = ChangeInfo {
            id: self.change_counter.to_string(),
            kind: format!("{action:?}").to_lowercase(),
            summary: format!("{action:?} {targets:?}"),
            status,
            spawn_time: now,
            ready_time: Some(now),
            err: err_msg,
        };
        self.changes.push(change.clone());
        change
    }

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
                    Some(Some(p)) => fmt_on_exit(p),
                    Some(None) => "done".into(), // oneshot
                    None => "restart".into(),
                };
                let on_failure = cfg
                    .map(crate::service::effective_on_failure)
                    .map(fmt_on_exit)
                    .unwrap_or_else(|| "restart".into());
                ServiceInfo {
                    name: e.snapshot.name.clone(),
                    startup: e.snapshot.startup,
                    current: e.snapshot.state.to_api_status(),
                    current_since: Some(e.snapshot.since),
                    stop_signal,
                    on_success,
                    on_failure,
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

    pub(super) async fn query_checks(&self, names: &[String]) -> Vec<vigil_types::api::CheckInfo> {
        let mut infos = Vec::new();
        for (name, entry) in &self.checks {
            if names.is_empty() || names.contains(name) {
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

    /// Hand a refresh to the check's actor, or run a check without one
    /// (`startup: disabled`) once in a task of its own. Never waits for the
    /// probe.
    pub(super) fn refresh_check(
        &self,
        name: String,
        reply: oneshot::Sender<Option<vigil_types::api::CheckRefresh>>,
    ) {
        let Some(config) = self.plan.checks.get(&name) else {
            let _ = reply.send(None);
            return;
        };
        if let Some(entry) = self.checks.get(&name) {
            let tx = entry.handle.tx.clone();
            tokio::spawn(async move {
                let (inner_tx, inner_rx) = oneshot::channel();
                if tx.send(check::Cmd::Refresh(inner_tx)).await.is_ok()
                    && let Ok(result) = inner_rx.await
                {
                    let _ = reply.send(Some(result));
                }
                // Otherwise the actor stopped meanwhile (replan): dropping
                // `reply` makes the request fail.
            });
            return;
        }
        let config = config.clone();
        let http = self.check_clients.get(&name).cloned();
        let svc_configs = std::sync::Arc::new(self.plan.services.clone());
        tokio::spawn(async move {
            let result = check::run_once_detached(name, config, http, svc_configs).await;
            let _ = reply.send(Some(result));
        });
    }

    fn query_alerts(&self, names: &[String]) -> Vec<AlertInfo> {
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

pub(super) fn fmt_on_exit(policy: OnExit) -> String {
    match policy {
        OnExit::Restart => "restart",
        OnExit::Ignore => "ignore",
        OnExit::Shutdown => "shutdown",
        OnExit::FailureShutdown => "failure-shutdown",
        OnExit::SuccessShutdown => "success-shutdown",
    }
    .into()
}
