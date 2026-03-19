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

    /// Returns `true` if all ordering dependencies of `name` are running.
    ///
    /// Three sources contribute:
    /// - `after: [X]`    — X must be running before name starts
    /// - `requires: [X]` — same ordering constraint (plus a stop cascade at runtime)
    /// - `X before: [name]` — X declared it must start before name (reverse `after`)
    fn after_deps_running(&self, name: &str) -> bool {
        let Some(config) = self.plan.services.get(name) else {
            return true;
        };

        // after: and requires: direct ordering deps
        let direct_ok = config.after.iter().chain(config.requires.iter()).all(|dep| {
            self.services
                .get(dep)
                .map(|e| e.snapshot.state.is_running())
                .unwrap_or(false)
        });

        // before: reverse — any service X where name ∈ X.before must be running first
        let before_ok = self
            .plan
            .services
            .iter()
            .filter(|(other, c)| *other != name && c.before.contains(&name.to_string()))
            .all(|(dep, _)| {
                self.services
                    .get(dep)
                    .map(|e| e.snapshot.state.is_running())
                    .unwrap_or(false)
            });

        direct_ok && before_ok
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
                                .map(|c| c.after.contains(&event.service) || c.requires.contains(&event.service))
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

                // requires: stop cascade — if a required dependency has permanently stopped
                // (Inactive or Error), stop every service that lists it in requires: and is
                // still active.  Backoff and Stopping are transient states: the service will
                // restart or finish stopping on its own, so we must not cascade there.
                if matches!(new_state, ServiceState::Inactive | ServiceState::Error) {
                    let dependents: Vec<String> = self
                        .plan
                        .services
                        .iter()
                        .filter(|(dep_name, c)| {
                            c.requires.contains(&event.service)
                                && self
                                    .services
                                    .get(*dep_name)
                                    .map(|e| e.snapshot.state.is_running())
                                    .unwrap_or(false)
                        })
                        .map(|(name, _)| name.clone())
                        .collect();
                    for dep_name in dependents {
                        warn!(
                            service = %dep_name,
                            requires = %event.service,
                            "required dependency stopped — stopping dependent service",
                        );
                        if let Err(e) = self.svc_cmd(&dep_name, SvcCmd::Stop).await {
                            error!(service = %dep_name, error = %e, "failed to stop service after required dependency stopped");
                        }
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use indexmap::IndexMap;
    use tokio::sync::mpsc;
    use vigil_types::plan::{Plan, ServiceConfig, Startup};

    use crate::logs::LogStore;
    use crate::metrics::MetricsStore;
    use crate::service::{self, Snapshot};
    use crate::state::ServiceState;

    // ------------------------------------------------------------------
    // Test helpers
    // ------------------------------------------------------------------

    /// Build a minimal `Overlord` for testing `after_deps_running` and
    /// `handle_svc_event`.
    ///
    /// `plan_services`: list of `(name, after, requires, before)` tuples
    /// that describe the dependency graph.
    ///
    /// `states`: current state for each service name (services not listed
    /// here get `ServiceState::Inactive`).
    fn make_overlord(
        plan_services: &[(&str, &[&str], &[&str], &[&str])],
        states: &[(&str, ServiceState)],
    ) -> Overlord {
        let mut services_plan: IndexMap<String, ServiceConfig> = IndexMap::new();
        for (name, after, requires, before) in plan_services {
            let mut cfg = ServiceConfig::default();
            cfg.after = after.iter().map(|s| s.to_string()).collect();
            cfg.requires = requires.iter().map(|s| s.to_string()).collect();
            cfg.before = before.iter().map(|s| s.to_string()).collect();
            services_plan.insert(name.to_string(), cfg);
        }

        let state_map: std::collections::HashMap<&str, ServiceState> =
            states.iter().copied().collect();

        let mut svc_map: IndexMap<String, ServiceEntry> = IndexMap::new();
        for (name, _, _, _) in plan_services {
            let (tx, _rx) = mpsc::channel(4);
            let handle = service::Handle { tx };
            let state = state_map.get(name).copied().unwrap_or(ServiceState::Inactive);
            let snapshot = Snapshot {
                name: name.to_string(),
                state,
                since: Utc::now(),
                startup: Startup::Enabled,
                pid: None,
            };
            svc_map.insert(name.to_string(), ServiceEntry { handle, snapshot });
        }

        let (event_tx, event_rx) = mpsc::channel(8);
        let (check_event_tx, check_event_rx) = mpsc::channel(8);
        let (_, cmd_rx) = mpsc::channel(8);

        Overlord {
            plan: Plan { services: services_plan, checks: IndexMap::new(), layers: Vec::new() },
            services: svc_map,
            checks: IndexMap::new(),
            changes: Vec::new(),
            change_counter: 0,
            boot_id: "test".to_string(),
            start_time: Utc::now(),
            http_address: String::new(),
            https_address: None,
            log_store: LogStore::new(64, 64),
            metrics: MetricsStore::new(),
            event_rx,
            event_tx,
            check_event_rx,
            check_event_tx,
            cmd_rx,
            layers_dir: std::path::PathBuf::new(),
            pending_autostart: Vec::new(),
        }
    }

    // ------------------------------------------------------------------
    // after_deps_running — `after:` ordering
    // ------------------------------------------------------------------

    #[test]
    fn after_dep_running_unblocks_service() {
        let ov = make_overlord(
            &[("dep", &[], &[], &[]), ("svc", &["dep"], &[], &[])],
            &[("dep", ServiceState::Active), ("svc", ServiceState::Inactive)],
        );
        assert!(ov.after_deps_running("svc"));
    }

    #[test]
    fn after_dep_inactive_blocks_service() {
        let ov = make_overlord(
            &[("dep", &[], &[], &[]), ("svc", &["dep"], &[], &[])],
            &[("dep", ServiceState::Inactive), ("svc", ServiceState::Inactive)],
        );
        assert!(!ov.after_deps_running("svc"));
    }

    #[test]
    fn service_with_no_deps_is_always_ready() {
        let ov = make_overlord(&[("svc", &[], &[], &[])], &[]);
        assert!(ov.after_deps_running("svc"));
    }

    // ------------------------------------------------------------------
    // after_deps_running — `before:` (reverse-after) ordering
    // ------------------------------------------------------------------

    #[test]
    fn before_dep_running_unblocks_service() {
        // "gate before: [svc]" means svc must wait for gate.
        let ov = make_overlord(
            &[("gate", &[], &[], &["svc"]), ("svc", &[], &[], &[])],
            &[("gate", ServiceState::Active), ("svc", ServiceState::Inactive)],
        );
        assert!(ov.after_deps_running("svc"));
    }

    #[test]
    fn before_dep_inactive_blocks_service() {
        let ov = make_overlord(
            &[("gate", &[], &[], &["svc"]), ("svc", &[], &[], &[])],
            &[("gate", ServiceState::Inactive), ("svc", ServiceState::Inactive)],
        );
        assert!(!ov.after_deps_running("svc"));
    }

    // ------------------------------------------------------------------
    // after_deps_running — `requires:` ordering (identical to `after:`)
    // ------------------------------------------------------------------

    #[test]
    fn requires_dep_running_unblocks_service() {
        let ov = make_overlord(
            &[("db", &[], &[], &[]), ("api", &[], &["db"], &[])],
            &[("db", ServiceState::Active), ("api", ServiceState::Inactive)],
        );
        assert!(ov.after_deps_running("api"));
    }

    #[test]
    fn requires_dep_inactive_blocks_service() {
        let ov = make_overlord(
            &[("db", &[], &[], &[]), ("api", &[], &["db"], &[])],
            &[("db", ServiceState::Inactive), ("api", ServiceState::Inactive)],
        );
        assert!(!ov.after_deps_running("api"));
    }

    // ------------------------------------------------------------------
    // requires: stop cascade
    // ------------------------------------------------------------------

    /// Helper: build an overlord with db (Active) and api (Active, requires db),
    /// wire both handles to auto-reply tasks, and return the overlord plus a
    /// receiver that signals whenever api receives a Stop command.
    async fn make_requires_overlord() -> (Overlord, mpsc::Receiver<()>) {
        let plan_services: &[(&str, &[&str], &[&str], &[&str])] =
            &[("db", &[], &[], &[]), ("api", &[], &["db"], &[])];
        let states = &[("db", ServiceState::Active), ("api", ServiceState::Active)];
        let mut ov = make_overlord(plan_services, states);

        let (stop_tx, stop_rx) = mpsc::channel::<()>(4);

        for (svc_name, notify) in [("api", Some(stop_tx)), ("db", None)] {
            let (tx, mut rx) = mpsc::channel::<service::Cmd>(8);
            let n = notify;
            tokio::spawn(async move {
                while let Some(cmd) = rx.recv().await {
                    match cmd {
                        service::Cmd::Stop(reply) => {
                            if let Some(ref t) = n { let _ = t.try_send(()); }
                            let _ = reply.send(Ok(()));
                        }
                        service::Cmd::Start(reply) => { let _ = reply.send(Ok(())); }
                        service::Cmd::Restart(reply) => { let _ = reply.send(Ok(())); }
                        _ => {}
                    }
                }
            });
            if let Some(entry) = ov.services.get_mut(svc_name) {
                entry.handle = service::Handle { tx };
            }
        }

        (ov, stop_rx)
    }

    #[tokio::test]
    async fn requires_stop_cascade_fires_on_inactive() {
        // db becomes Inactive → api (requires db) must be stopped.
        let (mut ov, mut stop_rx) = make_requires_overlord().await;
        let event = service::Event {
            service: "db".to_string(),
            kind: service::EventKind::StateChanged { new_state: ServiceState::Inactive },
        };
        ov.handle_svc_event(event).await;
        assert!(stop_rx.try_recv().is_ok(), "Stop was never sent to api on Inactive");
    }

    #[tokio::test]
    async fn requires_stop_cascade_fires_on_error() {
        // db enters Error → api (requires db) must also be stopped.
        let (mut ov, mut stop_rx) = make_requires_overlord().await;
        let event = service::Event {
            service: "db".to_string(),
            kind: service::EventKind::StateChanged { new_state: ServiceState::Error },
        };
        ov.handle_svc_event(event).await;
        assert!(stop_rx.try_recv().is_ok(), "Stop was never sent to api on Error");
    }

    #[tokio::test]
    async fn requires_stop_cascade_does_not_fire_on_backoff() {
        // db enters Backoff (transient crash, will restart) → api must NOT be stopped.
        let (mut ov, mut stop_rx) = make_requires_overlord().await;
        let event = service::Event {
            service: "db".to_string(),
            kind: service::EventKind::StateChanged { new_state: ServiceState::Backoff },
        };
        ov.handle_svc_event(event).await;
        assert!(stop_rx.try_recv().is_err(), "Stop was wrongly sent to api during Backoff");
    }

    #[tokio::test]
    async fn requires_stop_cascade_does_not_fire_on_stopping() {
        // db enters Stopping (transient) → cascade must not fire yet; it fires on Inactive.
        let (mut ov, mut stop_rx) = make_requires_overlord().await;
        let event = service::Event {
            service: "db".to_string(),
            kind: service::EventKind::StateChanged { new_state: ServiceState::Stopping },
        };
        ov.handle_svc_event(event).await;
        assert!(stop_rx.try_recv().is_err(), "Stop was wrongly sent to api during Stopping");
    }
}
