// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Autostart: which services start by themselves, and when their
//! dependencies let them.

use tracing::{error, info};

use crate::service::Cmd as SvcCmd;
use crate::state::ServiceState;

use super::Overlord;

impl Overlord {
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
        if let Some(why) = super::plan::dependencies::autostart_skip_reason(cfg) {
            return Some(why);
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

    /// Start each service now, or queue it until its dependencies are
    /// ready. Returns the start errors as `name: error`.
    pub(super) async fn start_or_defer(&mut self, names: Vec<String>) -> Vec<String> {
        let mut errors = Vec::new();
        for name in names {
            if self.ready_to_start(&name) {
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

    /// Start the queued services whose dependencies are now ready.
    pub(super) async fn try_start_pending(&mut self) {
        let ready: Vec<String> = self
            .pending_autostart
            .iter()
            .filter(|name| self.ready_to_start(name))
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

    /// Returns `true` if all dependencies of `name` are ready (`Active`, or
    /// `Done` for a oneshot; a service still `Starting` has not reached
    /// readiness).
    ///
    /// Three sources contribute:
    /// - `after: [X]`    — X must be ready before name starts
    /// - `requires: [X]` — same ordering constraint (plus a stop cascade at runtime)
    /// - `X before: [name]` — X declared it must start before name (reverse `after`)
    pub(super) fn ready_to_start(&self, name: &str) -> bool {
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
        let mut unmet: Vec<String> = Vec::new();
        for dep in super::plan::dependencies::ordering_deps(&self.plan, name, config) {
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

    /// Whether the service `name` is ready (active, or a finished oneshot).
    fn dep_ready(&self, name: &str) -> bool {
        self.services
            .get(name)
            .is_some_and(|e| e.snapshot.state.is_ready())
    }
}
