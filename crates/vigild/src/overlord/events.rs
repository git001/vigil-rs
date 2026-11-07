// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Events of the service actors: state changes, exits, health.

use chrono::Utc;
use tracing::{error, info, warn};

use crate::alert::{AlertEvent, AlertSource};
use crate::service::{self, Cmd as SvcCmd};
use crate::state::ServiceState;

use super::Overlord;

impl Overlord {
    /// Returns `Some(exit_code)` if a service policy requires daemon shutdown.
    pub(super) async fn handle_svc_event(&mut self, event: service::Event) -> Option<i32> {
        match &event.kind {
            service::EventKind::StateChanged { new_state } => {
                self.state_changed(&event.service, *new_state).await;
                None
            }
            service::EventKind::ProcessExited { exit, .. } => {
                if let Some(e) = self.services.get_mut(&event.service) {
                    e.snapshot.last_exit = *exit;
                }
                None
            }
            service::EventKind::NextRun { at } => {
                if let Some(e) = self.services.get_mut(&event.service) {
                    e.snapshot.next_run = *at;
                }
                None
            }
            service::EventKind::StatusMessage { message } => {
                if let Some(e) = self.services.get_mut(&event.service) {
                    e.snapshot.status_message = message.clone();
                }
                None
            }
            service::EventKind::Unhealthy {
                event: name,
                message,
            } => {
                self.healthy.remove(&event.service);
                warn!(service = %event.service, event = name, %message, "service failed");
                let ev = AlertEvent::firing(AlertSource::Service, &event.service, *name, message);
                self.send_alert(ev);
                None
            }
            service::EventKind::Healthy => {
                self.healthy.insert(event.service.clone());
                let ev = AlertEvent::resolved(
                    AlertSource::Service,
                    &event.service,
                    format!("service {} is healthy again", event.service),
                );
                self.send_alert(ev);
                // Report what its checks held back meanwhile.
                for check in self.checks_of_service(&event.service) {
                    if !self.check_quiet(&check) {
                        let fired = self.alert_sender.recheck(&check);
                        self.count_fired(&fired);
                    }
                }
                None
            }
            service::EventKind::DaemonShutdown { exit_code } => {
                info!(service = %event.service, exit_code, "daemon shutdown requested by service policy");
                Some(*exit_code)
            }
        }
    }

    /// A service changed its state: record it, start or stop what depends
    /// on it, and restart it if a replan stopped it.
    async fn state_changed(&mut self, service: &str, new_state: ServiceState) {
        // Healthy again only via `Healthy`; a finished oneshot keeps
        // the health its run gave it.
        if new_state != ServiceState::Done {
            self.healthy.remove(service);
        }
        if let Some(e) = self.services.get_mut(service) {
            e.snapshot.state = new_state;
            e.snapshot.since = Utc::now();
        }
        // When a service becomes ready (or a oneshot is done), unblock
        // any services waiting on it.
        if new_state.is_ready() && !self.pending_autostart.is_empty() {
            self.try_start_pending().await;
        }
        if new_state == ServiceState::Error {
            self.warn_blocked(service);
        }
        // requires: stop cascade — if a required dependency has permanently
        // stopped (Inactive or Error), stop every service that lists it in
        // requires: and is still active. Backoff and Stopping are transient
        // states: the service will restart or finish stopping on its own, so
        // we must not cascade there.
        if matches!(new_state, ServiceState::Inactive | ServiceState::Error) {
            self.stop_dependents(service).await;
        }

        self.restart_stopped(service, new_state).await;
    }

    /// `service` entered Error: warn about queued services that wait for it,
    /// they will never autostart.
    fn warn_blocked(&self, service: &str) {
        let blocked = self.blocked_by(service);
        if !blocked.is_empty() {
            warn!(
                dependency = %service,
                blocked = ?blocked,
                "dependency entered Error state; blocked services will never autostart"
            );
        }
    }

    /// Stop the running services that list `service` in `requires`.
    async fn stop_dependents(&mut self, service: &str) {
        let dependents: Vec<String> = self
            .plan
            .services
            .iter()
            .filter(|(dep_name, c)| {
                c.requires.iter().any(|r| r == service)
                    && self
                        .services
                        .get(*dep_name)
                        .is_some_and(|e| e.snapshot.state.is_running())
            })
            .map(|(name, _)| name.clone())
            .collect();
        for dep_name in dependents {
            warn!(
                service = %dep_name,
                requires = %service,
                "required dependency stopped — stopping dependent service",
            );
            if let Err(e) = self.svc_cmd(&dep_name, SvcCmd::Stop).await {
                error!(service = %dep_name, error = %e, "failed to stop service after required dependency stopped");
            }
        }
    }

    /// The queued services that wait for `service` (`after`, `requires`,
    /// or `service` lists them in `before`).
    pub(super) fn blocked_by(&self, service: &str) -> Vec<&str> {
        self.pending_autostart
            .iter()
            .filter(|name| self.unmet_deps(name).iter().any(|d| d == service))
            .map(String::as_str)
            .collect()
    }
}
