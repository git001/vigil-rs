// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Overlord lifecycle: actor synchronisation, autostart, reload, shutdown.

use std::sync::Arc;

use chrono::Utc;
use tokio::sync::oneshot;
use tracing::{error, info};
use vigil_types::api::CheckStatus;
use vigil_types::plan::Startup;

use crate::check;
use crate::service::{self, Cmd as SvcCmd};
use crate::state::ServiceState;

use super::{CheckEntry, Overlord, ServiceEntry};

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
                state: ServiceState::Inactive,
                since: Utc::now(),
                startup: config.startup,
                pid: None,
            };
            self.services.insert(
                name.clone(),
                ServiceEntry {
                    handle,
                    snapshot: snap,
                },
            );
        }

        // Keep vigil_service_info and vigil_services_count in sync.
        let names: Vec<&str> = self.services.keys().map(String::as_str).collect();
        self.metrics.set_services(&names);

        // Checks with startup: disabled are not auto-started.
        let svc_configs = Arc::new(self.plan.services.clone());
        for (name, config) in &self.plan.checks {
            if config.startup == Startup::Disabled {
                continue;
            }
            // Restart check actor if config changed since last (re)plan.
            if let Some(entry) = self.checks.get(name) {
                let old_json = serde_json::to_string(&entry.config).unwrap_or_default();
                let new_json = serde_json::to_string(config).unwrap_or_default();
                if old_json == new_json {
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
                },
            );
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
    pub(super) async fn try_start_pending(&mut self) {
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

    pub(super) async fn reload_layers(&mut self) -> anyhow::Result<()> {
        let dir = self.layers_dir.clone();
        self.plan = super::plan::load_plan(&dir)?;
        info!(
            services = self.plan.services.len(),
            checks = self.plan.checks.len(),
            alerts = self.plan.alerts.len(),
            "plan reloaded"
        );
        self.alert_sender.update_alerts(self.plan.alerts.clone());
        let queue_depth = self
            .plan
            .alert_queue_depth
            .unwrap_or(crate::alert::DEFAULT_DELIVERY_QUEUE);
        let max_age = self
            .plan
            .alert_max_queue_time
            .as_deref()
            .and_then(|s| crate::duration::parse_duration(s).ok())
            .unwrap_or(crate::alert::DEFAULT_DELIVERY_AGE);
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
        Ok(())
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
