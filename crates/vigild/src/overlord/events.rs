// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Overlord event handlers: service state transitions and check results.

use std::process;

use chrono::Utc;
use tracing::{error, info, warn};
use vigil_types::api::CheckStatus;
use vigil_types::plan::OnExit;

use crate::check::CheckEvent;
use crate::service::{self, Cmd as SvcCmd};
use crate::state::ServiceState;

use super::Overlord;

impl Overlord {
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
                                .map(|c| {
                                    c.after.contains(&event.service)
                                        || c.requires.contains(&event.service)
                                })
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
        let fired = self
            .alert_sender
            .handle_check_event(&event.check, event.status);
        for alert_name in &fired {
            self.metrics.record_alert_fire(alert_name);
        }

        if event.status != CheckStatus::Down {
            return;
        }

        let actions: Vec<(String, OnExit)> = self
            .plan
            .services
            .iter()
            .filter_map(|(name, cfg)| {
                cfg.on_check_failure
                    .get(&event.check)
                    .map(|a| (name.clone(), *a))
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
}
