// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Overlord lifecycle: actor synchronisation, autostart, reload, shutdown.

use std::sync::Arc;

use chrono::Utc;
use indexmap::IndexMap;
use tokio::sync::oneshot;
use tracing::{error, info, warn};
use vigil_types::api::CheckStatus;
use vigil_types::plan::Startup;

use crate::alert::{AlertEvent, AlertSource};
use crate::check;
use crate::service::{self, Cmd as SvcCmd};
use crate::state::ServiceState;

use super::{CheckEntry, Overlord, ServiceEntry};

impl Overlord {
    pub(super) fn sync_actors(&mut self) {
        for (name, config) in &self.plan.services {
            if let Some(entry) = self.services.get_mut(name) {
                // Existing actor: hand over the (possibly changed) config.
                entry.snapshot.startup = config.startup();
                if entry
                    .handle
                    .tx
                    .try_send(SvcCmd::UpdateConfig(Box::new(config.clone())))
                    .is_err()
                {
                    warn!(service = %name, "could not pass the new config to the service actor");
                }
                continue;
            }
            let handle = service::spawn(
                name.clone(),
                config.clone(),
                service::Context {
                    event_tx: self.event_tx.clone(),
                    log_store: Arc::clone(&self.log_store),
                    metrics: Arc::clone(&self.metrics),
                    exit_code_mode: self.exit_code_mode,
                    notify_dir: self.notify_dir.clone(),
                },
            );
            let snap = service::Snapshot {
                name: name.clone(),
                state: ServiceState::Inactive,
                since: Utc::now(),
                startup: config.startup(),
                last_exit: None,
                status_message: None,
                next_run: None,
            };
            self.services.insert(
                name.clone(),
                ServiceEntry {
                    handle,
                    snapshot: snap,
                },
            );
        }

        // Keep vigil_service_info and vigil_services_count in sync with the
        // plan (removed services are dropped from `self.services` later).
        let names: Vec<&str> = self.plan.services.keys().map(String::as_str).collect();
        self.metrics.set_services(&names);

        let svc_configs = Arc::new(self.plan.services.clone());
        for (name, config) in &self.plan.checks {
            // Checks with startup: disabled do not run; a replan to disabled
            // stops a running one.
            if config.startup() == Startup::Disabled {
                if let Some(entry) = self.checks.shift_remove(name) {
                    info!(check = %name, "check disabled, stopping its actor");
                    let _ = entry.handle.tx.try_send(check::Cmd::Shutdown);
                }
                continue;
            }
            let context = config
                .exec
                .as_ref()
                .and_then(|e| e.service_context.as_deref())
                .and_then(|s| self.plan.services.get(s))
                .cloned();
            // Restart the check actor if its config or its service context
            // changed since the last (re)plan.
            if let Some(entry) = self.checks.get(name) {
                if entry.config == *config && entry.context == context {
                    // Unchanged check: hand over the client this replan
                    // built, so a rotated tls.ca or client certificate
                    // takes effect without resetting the check's status.
                    if let Some(client) = self.check_clients.get(name)
                        && entry
                            .handle
                            .tx
                            .try_send(check::Cmd::SetHttpClient(client.clone()))
                            .is_err()
                    {
                        warn!(check = %name, "check actor busy, it keeps its previous HTTP client");
                    }
                    continue;
                }
                info!(check = %name, "check config changed, restarting check actor");
                let _ = entry.handle.tx.try_send(check::Cmd::Shutdown);
                self.checks.shift_remove(name);
            }
            let initial_status = self
                .alert_sender
                .check_status(name)
                .unwrap_or(CheckStatus::Up);
            let handle = check::spawn(
                name.clone(),
                config.clone(),
                self.check_clients.get(name).cloned(),
                Arc::clone(&svc_configs),
                self.check_event_tx.clone(),
                Arc::clone(&self.metrics),
                initial_status,
            );
            self.checks.insert(
                name.clone(),
                CheckEntry {
                    handle,
                    config: config.clone(),
                    context,
                },
            );
        }
    }

    /// Start every service autostart covers (at startup, on replan).
    pub(super) async fn autostart(&mut self) {
        let names = self.autostart_names();
        self.start_or_defer(names).await;
    }

    /// The services autostart covers, in plan order.
    pub(super) fn autostart_names(&self) -> Vec<String> {
        self.plan
            .services
            .keys()
            .filter(|n| self.autostart_skip_reason(n).is_none())
            .cloned()
            .collect()
    }

    /// Why autostart leaves `name` alone; `None` if it starts it.
    pub(super) fn autostart_skip_reason(&self, name: &str) -> Option<&'static str> {
        let Some(cfg) = self.plan.services.get(name) else {
            return Some("not in the plan");
        };
        if cfg.startup() != Startup::Enabled {
            return Some("startup is not enabled");
        }
        // Scheduled jobs are started by their schedule, not at startup.
        if cfg.schedule.is_some() {
            return Some("scheduled; its schedule starts it");
        }
        // A finished oneshot is not run again by a replan.
        if self
            .services
            .get(name)
            .is_some_and(|e| e.snapshot.state == ServiceState::Done)
        {
            return Some("oneshot has already run");
        }
        None
    }

    /// Start each service now, or queue it until its `after` dependencies
    /// are ready. Returns the start errors as `name: error`.
    pub(super) async fn start_or_defer(&mut self, names: Vec<String>) -> Vec<String> {
        let mut errors = Vec::new();
        for name in names {
            if self.after_deps_running(&name) {
                if let Err(e) = self.svc_cmd(&name, SvcCmd::Start).await {
                    error!(service = %name, error = %e, "autostart failed");
                    errors.push(format!("{name}: {e:#}"));
                }
            } else {
                info!(service = %name, "deferring autostart: waiting for dependencies to become ready");
                if !self.pending_autostart.contains(&name) {
                    self.pending_autostart.push(name);
                }
            }
        }
        errors
    }

    /// Returns `true` if all ordering dependencies of `name` are ready
    /// (`Active`, or `Done` for a oneshot; a service still `Starting` has not
    /// reached readiness).
    ///
    /// Three sources contribute:
    /// - `after: [X]`    — X must be running before name starts
    /// - `requires: [X]` — same ordering constraint (plus a stop cascade at runtime)
    /// - `X before: [name]` — X declared it must start before name (reverse `after`)
    pub(super) fn after_deps_running(&self, name: &str) -> bool {
        self.unmet_deps(name).is_empty()
    }

    /// The dependencies `name` still waits for before it starts.
    ///
    /// `after` and `before` only order services that are started anyway: a
    /// dependency autostart does not start (disabled, scheduled) is ignored
    /// unless it is starting right now, so it cannot block the service
    /// forever. `requires` is a hard dependency: it is always waited for.
    pub(super) fn unmet_deps(&self, name: &str) -> Vec<String> {
        let Some(config) = self.plan.services.get(name) else {
            return Vec::new();
        };
        // X before: [name] makes X an ordering dependency of name.
        let reverse_before = self
            .plan
            .services
            .iter()
            .filter(|(other, c)| *other != name && c.before.iter().any(|b| b == name))
            .map(|(other, _)| other);
        let ordering = config.after.iter().chain(reverse_before);

        let mut unmet: Vec<String> = Vec::new();
        for dep in ordering {
            if !self.dep_ready(dep) && !self.ordering_dep_ignored(dep) && !unmet.contains(dep) {
                unmet.push(dep.clone());
            }
        }
        for dep in &config.requires {
            if !self.dep_ready(dep) && !unmet.contains(dep) {
                unmet.push(dep.clone());
            }
        }
        unmet
    }

    /// An `after`/`before` dependency that will not come up by itself: not
    /// started by autostart and not starting now.
    fn ordering_dep_ignored(&self, dep: &str) -> bool {
        self.autostart_skip_reason(dep).is_some()
            && self
                .services
                .get(dep)
                .is_none_or(|e| e.snapshot.state != ServiceState::Starting)
    }

    /// Check pending services and start any whose `after:` deps are now running.
    pub(super) async fn try_start_pending(&mut self) {
        let ready: Vec<String> = self
            .pending_autostart
            .iter()
            .filter(|name| self.after_deps_running(name))
            .cloned()
            .collect();

        for name in ready {
            self.pending_autostart.retain(|n| n != &name);
            info!(service = %name, "starting deferred service: dependencies are ready");
            if let Err(e) = self.svc_cmd(&name, SvcCmd::Start).await {
                error!(service = %name, error = %e, "deferred autostart failed");
            }
        }
    }

    /// Load the layers again and apply them (`vigil replan`,
    /// `--watch-layers`). Invalid layers leave the running plan as it is and
    /// send the alert `vigild`/`layers`/`invalid`; the next successful
    /// reload resolves it.
    pub(super) async fn reload_layers(&mut self) -> anyhow::Result<()> {
        let dir = self.layers_dir.clone();
        // Reading files off the async runtime: a hanging file system must
        // not block the overlord.
        let loaded = crate::template::off_runtime("layers", move || super::plan::load(&dir)).await;
        match loaded {
            Ok(loaded) => {
                self.apply_plan(loaded).await;
                self.metrics.record_layers_reload(true);
                self.send_alert(AlertEvent::resolved(
                    AlertSource::Vigild,
                    "layers",
                    "layers loaded",
                ));
                Ok(())
            }
            Err(e) => {
                error!(error = %format!("{e:#}"), "layers are invalid, keeping the running plan");
                self.metrics.record_layers_reload(false);
                self.send_alert(AlertEvent::firing(
                    AlertSource::Vigild,
                    "layers",
                    "invalid",
                    format!("{e:#}"),
                ));
                Err(e)
            }
        }
    }

    async fn apply_plan(&mut self, loaded: super::plan::Loaded) {
        let super::plan::Loaded {
            plan,
            alerts,
            checks,
            identities,
            files,
        } = loaded;
        self.check_clients = checks;
        self.identity_store.set_layer(identities).await;
        self.watched_files.set(files);
        let old_plan = std::mem::replace(&mut self.plan, plan);

        let services_changed = count_changed(&old_plan.services, &self.plan.services);
        // Decided before sync_actors: it needs the states from before the
        // replan.
        let restarts = self.replan_restarts(&old_plan.services);
        let checks_changed = count_changed(&old_plan.checks, &self.plan.checks);
        let alerts_changed = count_changed(&old_plan.alerts, &self.plan.alerts);
        info!(
            services = self.plan.services.len(),
            checks = self.plan.checks.len(),
            alerts = self.plan.alerts.len(),
            services_changed,
            checks_changed,
            alerts_changed,
            restarted = ?restarts,
            "plan reloaded"
        );
        self.alert_sender.update_alerts(alerts);
        let (queue_depth, max_age) = crate::alert::queue_limits(&self.plan);
        self.alert_sender.update_queue_limits(queue_depth, max_age);
        self.sync_actors();
        self.pending_autostart.clear();
        let plan = &self.plan;
        self.pending_restart.retain(|n| plan.services.contains_key(n));

        let removed_svcs: Vec<String> = self
            .services
            .keys()
            .filter(|n| !self.plan.services.contains_key(*n))
            .cloned()
            .collect();
        for name in removed_svcs {
            self.healthy.remove(&name);
            if let Some(e) = self.services.shift_remove(&name) {
                let _ = e.handle.tx.send(SvcCmd::Shutdown).await;
            }
        }

        let removed_chks: Vec<String> = self
            .checks
            .keys()
            .filter(|n| !self.plan.checks.contains_key(*n))
            .cloned()
            .collect();
        for name in removed_chks {
            if let Some(e) = self.checks.shift_remove(&name) {
                let _ = e.handle.tx.send(check::Cmd::Shutdown).await;
            }
        }

        self.autostart().await;
        self.begin_restarts(restarts).await;
    }

    /// Whether the service `name` is ready (active, or a finished oneshot).
    fn dep_ready(&self, name: &str) -> bool {
        self.services
            .get(name)
            .is_some_and(|e| e.snapshot.state.is_ready())
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

/// Entries present in both maps whose config differs (for the replan log).
fn count_changed<V: PartialEq>(old: &IndexMap<String, V>, new: &IndexMap<String, V>) -> usize {
    new.iter()
        .filter(|(name, cfg)| old.get(*name).is_some_and(|o| o != *cfg))
        .count()
}
