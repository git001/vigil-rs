// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Replan restarts: a running service whose config changed is stopped and
//! started again with the new config, and so is every running service that
//! needs it via `requires`.
//!
//! The overlord never waits for a stop. It sends `Stop`, remembers the
//! service in `pending_restart` and starts it again when the service
//! reports `inactive`, through `start_or_defer`, so a dependent waits until
//! the service it requires is ready again.

use indexmap::IndexMap;
use tracing::{error, info};
use vigil_types::plan::ServiceConfig;

use crate::service::Cmd as SvcCmd;
use crate::state::ServiceState;

use super::Overlord;

impl Overlord {
    /// The services a replan from `old` restarts, in plan order: running
    /// services whose config changed in more than `summary` or
    /// `description`, plus the running services that require one of them,
    /// directly or through others.
    pub(super) fn replan_restarts(&self, old: &IndexMap<String, ServiceConfig>) -> Vec<String> {
        let mut restart: Vec<&String> = self
            .plan
            .services
            .iter()
            .filter(|(name, new)| {
                old.get(*name).is_some_and(|o| process_changed(o, new)) && self.restartable(name)
            })
            .map(|(name, _)| name)
            .collect();
        let mut i = 0;
        while i < restart.len() {
            let required = restart[i];
            for (name, config) in &self.plan.services {
                if config.requires.contains(required)
                    && !restart.contains(&name)
                    && self.restartable(name)
                {
                    restart.push(name);
                }
            }
            i += 1;
        }
        self.plan
            .services
            .keys()
            .filter(|name| restart.contains(name))
            .cloned()
            .collect()
    }

    /// A service with a process or about to get one again (`starting`,
    /// `active`, `backoff`). Scheduled jobs are left alone: a running run
    /// finishes with its old config, the next run gets the new one.
    fn restartable(&self, name: &str) -> bool {
        self.plan
            .services
            .get(name)
            .is_some_and(|c| c.schedule.is_none())
            && self.services.get(name).is_some_and(|e| {
                matches!(
                    e.snapshot.state,
                    ServiceState::Starting | ServiceState::Active | ServiceState::Backoff
                )
            })
    }

    /// Stop each of `names`; `restart_stopped` starts it again once it is
    /// stopped.
    pub(super) async fn begin_restarts(&mut self, names: Vec<String>) {
        for name in names {
            info!(service = %name, "config changed, restarting the service");
            if let Err(e) = self.svc_cmd(&name, SvcCmd::Stop).await {
                error!(service = %name, error = %format!("{e:#}"), "could not stop the service for its restart");
                continue;
            }
            if !self.pending_restart.contains(&name) {
                self.pending_restart.push(name);
            }
        }
    }

    /// `name` changed to `state`. If it waits for its replan restart and has
    /// stopped, start it, or queue it until its dependencies are ready.
    pub(super) async fn restart_stopped(&mut self, name: &str, state: ServiceState) {
        if state != ServiceState::Inactive {
            return;
        }
        let Some(pos) = self.pending_restart.iter().position(|n| n == name) else {
            return;
        };
        self.pending_restart.remove(pos);
        let errors = self.start_or_defer(vec![name.to_owned()]).await;
        for e in errors {
            error!(error = %e, "restart with the new config failed");
        }
    }
}

/// Whether the change from `old` to `new` concerns the process, that is
/// anything but `summary` and `description`.
fn process_changed(old: &ServiceConfig, new: &ServiceConfig) -> bool {
    let strip = |c: &ServiceConfig| ServiceConfig {
        summary: None,
        description: None,
        ..c.clone()
    };
    strip(old) != strip(new)
}
