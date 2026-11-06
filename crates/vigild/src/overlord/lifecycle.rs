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
                pid: None,
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

    pub(super) async fn autostart(&mut self) {
        let names: Vec<String> = self
            .plan
            .services
            .iter()
            // Scheduled jobs are started by their schedule, not at startup.
            .filter(|(_, c)| c.startup() == Startup::Enabled && c.schedule.is_none())
            // A finished oneshot is not run again by a replan.
            .filter(|(n, _)| {
                self.services
                    .get(*n)
                    .is_none_or(|e| e.snapshot.state != ServiceState::Done)
            })
            .map(|(n, _)| n.clone())
            .collect();

        for name in names {
            if self.after_deps_running(&name) {
                if let Err(e) = self.svc_cmd(&name, SvcCmd::Start).await {
                    error!(service = %name, error = %e, "autostart failed");
                }
            } else {
                info!(service = %name, "deferring autostart: waiting for dependencies to become ready");
                if !self.pending_autostart.contains(&name) {
                    self.pending_autostart.push(name);
                }
            }
        }
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
        let Some(config) = self.plan.services.get(name) else {
            return true;
        };

        // after: and requires: direct ordering deps
        let direct_ok = config
            .after
            .iter()
            .chain(config.requires.iter())
            .all(|dep| {
                self.services
                    .get(dep)
                    .map(|e| e.snapshot.state.is_ready())
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
                    .map(|e| e.snapshot.state.is_ready())
                    .unwrap_or(false)
            });

        direct_ok && before_ok
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
        let loaded =
            crate::template::off_runtime("layers", move || super::plan::load_plan(&dir)).await;
        match loaded {
            Ok(plan) => {
                self.apply_plan(plan).await;
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

    async fn apply_plan(&mut self, plan: vigil_types::plan::Plan) {
        let old_plan = std::mem::replace(&mut self.plan, plan);

        let services_changed = count_changed(&old_plan.services, &self.plan.services);
        let checks_changed = count_changed(&old_plan.checks, &self.plan.checks);
        let alerts_changed = count_changed(&old_plan.alerts, &self.plan.alerts);
        info!(
            services = self.plan.services.len(),
            checks = self.plan.checks.len(),
            alerts = self.plan.alerts.len(),
            services_changed,
            checks_changed,
            alerts_changed,
            "plan reloaded"
        );
        self.alert_sender.update_alerts(self.plan.alerts.clone());
        let (queue_depth, max_age) = crate::alert::queue_limits(&self.plan);
        self.alert_sender.update_queue_limits(queue_depth, max_age);
        self.sync_actors();
        self.pending_autostart.clear();

        let current_svcs: Vec<String> = self.plan.services.keys().cloned().collect();
        let removed_svcs: Vec<String> = self
            .services
            .keys()
            .filter(|n| !current_svcs.contains(n))
            .cloned()
            .collect();
        for name in removed_svcs {
            self.healthy.remove(&name);
            if let Some(e) = self.services.shift_remove(&name) {
                let _ = e.handle.tx.send(SvcCmd::Shutdown).await;
            }
        }

        let current_chks: Vec<String> = self.plan.checks.keys().cloned().collect();
        let removed_chks: Vec<String> = self
            .checks
            .keys()
            .filter(|n| !current_chks.contains(n))
            .cloned()
            .collect();
        for name in removed_chks {
            if let Some(e) = self.checks.shift_remove(&name) {
                let _ = e.handle.tx.send(check::Cmd::Shutdown).await;
            }
        }

        self.autostart().await;
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
