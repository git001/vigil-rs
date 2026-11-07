// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Commands that change something: service actions (`POST /v1/services`),
//! signals, and running a check now.

use chrono::Utc;
use tokio::sync::oneshot;
use vigil_types::api::{ChangeInfo, ChangeStatus};

use crate::check;
use crate::service::Cmd as SvcCmd;

use super::Overlord;

impl Overlord {
    pub(super) async fn handle_services_action(
        &mut self,
        action: vigil_types::api::ServiceAction,
        names: Vec<String>,
    ) -> anyhow::Result<ChangeInfo> {
        use vigil_types::api::ServiceAction;

        match action {
            ServiceAction::Autostart => {
                let (targets, errors) = self.autostart_action(names).await;
                return Ok(self.record_change(action, &targets, errors));
            }
            // The layers are loaded once, whatever `names` says.
            ServiceAction::Replan => {
                let errors = match self.reload_layers().await {
                    Ok(()) => Vec::new(),
                    Err(e) => vec![format!("{e:#}")],
                };
                return Ok(self.record_change(action, &names, errors));
            }
            ServiceAction::Start | ServiceAction::Stop | ServiceAction::Restart => {}
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
                ServiceAction::Stop => {
                    // A stop wins over a pending replan restart.
                    self.pending_restart.retain(|n| n != name);
                    self.svc_cmd(name, SvcCmd::Stop).await
                }
                ServiceAction::Restart => self.svc_cmd(name, SvcCmd::Restart).await,
                ServiceAction::Autostart | ServiceAction::Replan => unreachable!("handled above"),
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

    /// `POST /v1/signals`: check every name first, so that either all
    /// services get the signal or none; then send it from a task of its own
    /// (a busy service actor must not block the overlord). A process that
    /// ended meanwhile is reported as not running.
    pub(super) fn signal_services(
        &self,
        signal: nix::sys::signal::Signal,
        services: Vec<String>,
        reply: oneshot::Sender<Result<Vec<String>, super::SignalError>>,
    ) {
        use super::SignalError;
        if services.is_empty() {
            let _ = reply.send(Err(SignalError::NoServices));
            return;
        }
        let mut targets = Vec::with_capacity(services.len());
        let mut not_running = Vec::new();
        for name in &services {
            let Some(entry) = self.services.get(name) else {
                let _ = reply.send(Err(SignalError::NotFound(name.clone())));
                return;
            };
            if entry.snapshot.state.has_process() {
                if !targets.iter().any(|(n, _)| n == name) {
                    targets.push((name.clone(), entry.handle.tx.clone()));
                }
            } else if !not_running.contains(name) {
                not_running.push(name.clone());
            }
        }
        if !not_running.is_empty() {
            let _ = reply.send(Err(SignalError::NotRunning(not_running)));
            return;
        }
        tokio::spawn(async move {
            let mut sent = Vec::new();
            let mut gone = Vec::new();
            for (name, tx) in targets {
                let (got_tx, got_rx) = oneshot::channel();
                let delivered = tx.send(SvcCmd::Signal(signal, got_tx)).await.is_ok()
                    && got_rx.await.unwrap_or(false);
                if delivered {
                    sent.push(name);
                } else {
                    gone.push(name);
                }
            }
            let result = if gone.is_empty() {
                Ok(sent)
            } else {
                Err(SignalError::NotRunning(gone))
            };
            let _ = reply.send(result);
        });
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
}
