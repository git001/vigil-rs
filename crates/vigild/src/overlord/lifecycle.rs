// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Overlord lifecycle: reload, shutdown, commands to the service actors.

use indexmap::IndexMap;
use tokio::sync::oneshot;
use tracing::{error, info};

use crate::alert::{AlertEvent, AlertSource};
use crate::check;
use crate::service::Cmd as SvcCmd;

use super::Overlord;

impl Overlord {
    /// Load the layers again and apply them (`vigil replan`,
    /// `--watch-layers`). Invalid layers leave the running plan as it is and
    /// send the alert `vigild`/`layers`/`invalid`; the next successful
    /// reload resolves it.
    pub(super) async fn reload_layers(&mut self) -> anyhow::Result<()> {
        let dir = self.layers_dir.clone();
        let notify = self.notify_dir.clone();
        // Reading files off the async runtime: a hanging file system must
        // not block the overlord.
        let loaded =
            crate::template::off_runtime("layers", move || super::plan::load(&dir, &notify)).await;
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
        self.pending_restart
            .retain(|n| plan.services.contains_key(n));
        self.drop_removed().await;

        self.autostart().await;
        self.begin_restarts(restarts).await;
    }

    pub(super) async fn stop_all(&mut self) {
        let mut set = tokio::task::JoinSet::new();
        for e in self.services.values() {
            let tx = e.handle.tx.clone();
            set.spawn(async move {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = tx.send(SvcCmd::Stop(reply_tx)).await;
                let _ = reply_rx.await;
                let _ = tx.send(SvcCmd::Shutdown).await;
            });
        }
        while set.join_next().await.is_some() {}

        for e in self.checks.values() {
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
