// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::process;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::oneshot;
use tracing::{error, info, warn};
use vigil_types::api::{ChangeInfo, ChangeStatus, CheckStatus, ServiceInfo, SystemInfo};
use vigil_types::plan::{OnExit, Startup};

use crate::state::ServiceState;

use crate::check::{self, CheckEvent};
use crate::service::{self, Cmd as SvcCmd};

use super::{CheckEntry, Cmd, Overlord, ServiceEntry};

impl Overlord {
    pub(super) fn sync_actors(&mut self) {
        for (name, config) in &self.plan.services {
            if self.services.contains_key(name) {
                continue;
            }
            let handle = service::spawn(
                name.clone(),
                config.clone(),
                self.event_tx.clone(),
                Arc::clone(&self.log_store),
                Arc::clone(&self.metrics),
            );
            let snap = service::Snapshot {
                name: name.clone(),
                state: crate::state::ServiceState::Inactive,
                since: Utc::now(),
                startup: config.startup,
                pid: None,
            };
            self.services.insert(name.clone(), ServiceEntry { handle, snapshot: snap });
        }

        // Keep vigil_service_info and vigil_services_count in sync.
        let names: Vec<&str> = self.services.keys().map(String::as_str).collect();
        self.metrics.set_services(&names);

        // Checks with startup: disabled are not auto-started.
        let svc_configs = Arc::new(self.plan.services.clone());
        for (name, config) in &self.plan.checks {
            if self.checks.contains_key(name) || config.startup == Startup::Disabled {
                continue;
            }
            let handle = check::spawn(
                name.clone(),
                config.clone(),
                Arc::clone(&svc_configs),
                self.check_event_tx.clone(),
                Arc::clone(&self.metrics),
            );
            self.checks.insert(name.clone(), CheckEntry { handle });
        }
    }

    pub(super) async fn autostart(&mut self) {
        let names: Vec<String> = self
            .plan
            .services
            .iter()
            .filter(|(_, c)| c.startup == Startup::Enabled)
            .map(|(n, _)| n.clone())
            .collect();

        for name in names {
            if self.after_deps_running(&name) {
                if let Err(e) = self.svc_cmd(&name, SvcCmd::Start).await {
                    error!(service = %name, error = %e, "autostart failed");
                }
            } else {
                info!(service = %name, "deferring autostart: waiting for 'after' dependencies");
                if !self.pending_autostart.contains(&name) {
                    self.pending_autostart.push(name);
                }
            }
        }
    }

    /// Returns `true` if all `after:` dependencies of `name` are running.
    fn after_deps_running(&self, name: &str) -> bool {
        let Some(config) = self.plan.services.get(name) else {
            return true;
        };
        config.after.iter().all(|dep| {
            self.services
                .get(dep)
                .map(|e| e.snapshot.state.is_running())
                .unwrap_or(false)
        })
    }

    /// Check pending services and start any whose `after:` deps are now running.
    async fn try_start_pending(&mut self) {
        let ready: Vec<String> = self
            .pending_autostart
            .iter()
            .filter(|name| self.after_deps_running(name))
            .cloned()
            .collect();

        for name in ready {
            self.pending_autostart.retain(|n| n != &name);
            info!(service = %name, "starting deferred service: 'after' dependencies are now running");
            if let Err(e) = self.svc_cmd(&name, SvcCmd::Start).await {
                error!(service = %name, error = %e, "deferred autostart failed");
            }
        }
    }

    pub(super) async fn handle_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Services { action, names, reply } => {
                let _ = reply.send(self.handle_services_action(action, names).await);
            }
            Cmd::GetServices { names, reply } => {
                let _ = reply.send(self.get_service_infos(&names));
            }
            Cmd::GetChanges { id, reply } => {
                let result = match id {
                    None => self.changes.clone(),
                    Some(ref id) => self.changes.iter().filter(|c| &c.id == id).cloned().collect(),
                };
                let _ = reply.send(result);
            }
            Cmd::GetChecks { names, reply } => {
                let infos = self.query_checks(&names).await;
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
                ServiceAction::Autostart => {
                    if let Some(svc) = self.plan.services.get_mut(name) {
                        svc.startup = Startup::Enabled;
                    }
                    self.svc_cmd(name, SvcCmd::Start).await
                }
                ServiceAction::Replan => self.reload_layers().await,
            };
            if let Err(e) = res {
                errors.push(format!("{}: {}", name, e));
            }
        }

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
        Ok(change)
    }

    fn get_service_infos(&self, names: &[String]) -> Vec<ServiceInfo> {
        self.services
            .values()
            .filter(|e| names.is_empty() || names.contains(&e.snapshot.name))
            .map(|e| {
                let cfg = self.plan.services.get(&e.snapshot.name);
                let stop_signal = cfg
                    .and_then(|c| c.stop_signal)
                    .map(|s| format!("{:?}", s.0))
                    .unwrap_or_else(|| "SIGTERM".into());
                let on_success = cfg
                    .and_then(|c| c.on_success)
                    .map(fmt_on_exit)
                    .unwrap_or_else(|| "restart".into());
                let on_failure = cfg
                    .and_then(|c| c.on_failure)
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
                }
            })
            .collect()
    }

    async fn query_checks(&self, names: &[String]) -> Vec<vigil_types::api::CheckInfo> {
        let mut infos = Vec::new();
        for (name, entry) in &self.checks {
            if names.is_empty() || names.contains(name) {
                let (tx, rx) = oneshot::channel();
                if entry.handle.tx.send(check::Cmd::GetStatus(tx)).await.is_ok() {
                    if let Ok(info) = rx.await {
                        infos.push(info);
                    }
                }
            }
        }
        infos
    }

    pub(super) async fn reload_layers(&mut self) -> anyhow::Result<()> {
        let dir = self.layers_dir.clone();
        self.plan = super::plan::load_plan(&dir)?;
        self.sync_actors();
        self.pending_autostart.clear();

        let current_svcs: Vec<String> = self.plan.services.keys().cloned().collect();
        let removed_svcs: Vec<String> =
            self.services.keys().filter(|n| !current_svcs.contains(n)).cloned().collect();
        for name in removed_svcs {
            if let Some(e) = self.services.shift_remove(&name) {
                let _ = e.handle.tx.send(SvcCmd::Shutdown).await;
            }
        }

        let current_chks: Vec<String> = self.plan.checks.keys().cloned().collect();
        let removed_chks: Vec<String> =
            self.checks.keys().filter(|n| !current_chks.contains(n)).cloned().collect();
        for name in removed_chks {
            if let Some(e) = self.checks.shift_remove(&name) {
                let _ = e.handle.tx.send(check::Cmd::Shutdown).await;
            }
        }

        self.autostart().await;
        Ok(())
    }

    /// Returns `Some(exit_code)` if a service policy requires daemon shutdown.
    pub(super) async fn handle_svc_event(&mut self, event: service::Event) -> Option<i32> {
        match &event.kind {
            service::EventKind::StateChanged { new_state } => {
                if let Some(e) = self.services.get_mut(&event.service) {
                    e.snapshot.state = *new_state;
                    e.snapshot.since = Utc::now();
                }
                // When a service starts running, unblock any services waiting on it.
                if new_state.is_running() && !self.pending_autostart.is_empty() {
                    self.try_start_pending().await;
                }
                // Warn if a service enters Error and others are still waiting for it.
                if *new_state == ServiceState::Error {
                    let blocked: Vec<&str> = self
                        .pending_autostart
                        .iter()
                        .filter(|name| {
                            self.plan
                                .services
                                .get(*name)
                                .map(|c| c.after.contains(&event.service))
                                .unwrap_or(false)
                        })
                        .map(String::as_str)
                        .collect();
                    if !blocked.is_empty() {
                        warn!(
                            dependency = %event.service,
                            blocked = ?blocked,
                            "dependency entered Error state; blocked services will never autostart"
                        );
                    }
                }
                None
            }
            service::EventKind::ProcessExited { .. } => None,
            service::EventKind::DaemonShutdown { exit_code } => {
                info!(service = %event.service, exit_code, "daemon shutdown requested by service policy");
                Some(*exit_code)
            }
        }
    }

    pub(super) async fn handle_check_event(&mut self, event: CheckEvent) {
        if event.status != CheckStatus::Down {
            return;
        }

        let actions: Vec<(String, OnExit)> = self
            .plan
            .services
            .iter()
            .filter_map(|(name, cfg)| {
                cfg.on_check_failure.get(&event.check).map(|a| (name.clone(), *a))
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
                OnExit::Shutdown => {
                    info!(service = %svc_name, check = %event.check, "on-check-failure: shutdown");
                    self.stop_all().await;
                    process::exit(10);
                }
                OnExit::SuccessShutdown => {
                    info!(service = %svc_name, check = %event.check, "on-check-failure: success-shutdown");
                    self.stop_all().await;
                    process::exit(0);
                }
                OnExit::FailureShutdown => {
                    info!(service = %svc_name, check = %event.check, "on-check-failure: failure-shutdown");
                    self.stop_all().await;
                    process::exit(10);
                }
            }
        }
    }

    pub(super) async fn stop_all(&mut self) {
        let names: Vec<String> = self.services.keys().cloned().collect();
        let mut set = tokio::task::JoinSet::new();
        for name in &names {
            if let Some(e) = self.services.get(name) {
                let tx = e.handle.tx.clone();
                set.spawn(async move {
                    let (reply_tx, reply_rx) = oneshot::channel();
                    let _ = tx.send(SvcCmd::Stop(reply_tx)).await;
                    let _ = reply_rx.await;
                    let _ = tx.send(SvcCmd::Shutdown).await;
                });
            }
        }
        while set.join_next().await.is_some() {}

        for (_, e) in &self.checks {
            let _ = e.handle.tx.send(check::Cmd::Shutdown).await;
        }
    }

    pub(super) async fn forward_signal(&self, signal: nix::sys::signal::Signal) {
        for entry in self.services.values() {
            let _ = entry.handle.tx.send(SvcCmd::ForwardSignal(signal)).await;
        }
    }

    pub(super) async fn svc_cmd(
        &self,
        name: &str,
        build: impl FnOnce(oneshot::Sender<anyhow::Result<()>>) -> SvcCmd,
    ) -> anyhow::Result<()> {
        let entry = self
            .services
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown service '{}'", name))?;
        let (reply_tx, reply_rx) = oneshot::channel();
        entry
            .handle
            .tx
            .send(build(reply_tx))
            .await
            .map_err(|_| anyhow::anyhow!("service actor '{}' gone", name))?;
        reply_rx
            .await
            .map_err(|_| anyhow::anyhow!("service actor '{}' dropped reply", name))?
    }
}

fn fmt_on_exit(policy: OnExit) -> String {
    match policy {
        OnExit::Restart => "restart",
        OnExit::Ignore => "ignore",
        OnExit::Shutdown => "shutdown",
        OnExit::FailureShutdown => "failure-shutdown",
        OnExit::SuccessShutdown => "success-shutdown",
    }
    .into()
}
