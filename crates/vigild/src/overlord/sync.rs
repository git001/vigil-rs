// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Keep the service and check actors in step with the plan.

use std::sync::Arc;

use indexmap::IndexMap;
use tracing::{info, warn};
use vigil_types::api::CheckStatus;
use vigil_types::plan::Startup;

use crate::check;
use crate::service::{self, Cmd as SvcCmd};

use super::{CheckEntry, Overlord, ServiceEntry};

impl Overlord {
    /// Spawn an actor for every new service and check, hand the changed
    /// config to existing service actors, and restart changed checks.
    pub(super) fn sync_actors(&mut self) {
        self.sync_services();

        // Keep vigil_service_info and vigil_services_count in sync with the
        // plan (removed services are dropped from `self.services` later).
        let names: Vec<&str> = self.plan.services.keys().map(String::as_str).collect();
        self.metrics.set_services(&names);

        self.publish_running_checks();
        self.sync_checks();
        self.publish_health();
    }

    fn sync_services(&mut self) {
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
            let snapshot = service::Snapshot::inactive(name.clone(), config.startup());
            self.services
                .insert(name.clone(), ServiceEntry { handle, snapshot });
        }
    }

    fn sync_checks(&mut self) {
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

    /// Shut down the actors of services and checks the plan no longer has.
    pub(super) async fn drop_removed(&mut self) {
        for name in not_in(&self.services, &self.plan.services) {
            self.healthy.remove(&name);
            if let Some(e) = self.services.shift_remove(&name) {
                let _ = e.handle.tx.send(SvcCmd::Shutdown).await;
            }
        }
        for name in not_in(&self.checks, &self.plan.checks) {
            if let Some(e) = self.checks.shift_remove(&name) {
                let _ = e.handle.tx.send(check::Cmd::Shutdown).await;
            }
        }
    }
}

/// The names in `actors` that `plan` does not have.
fn not_in<A, P>(actors: &IndexMap<String, A>, plan: &IndexMap<String, P>) -> Vec<String> {
    actors
        .keys()
        .filter(|n| !plan.contains_key(*n))
        .cloned()
        .collect()
}
