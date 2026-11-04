// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Service health for alerts: failures (`Unhealthy`) and recovery
//! (`Healthy`, after `healthy-time` in `active`).

use std::time::Duration;

use tokio::time::Instant;
use tracing::error;
use vigil_types::api::ExitInfo;

use crate::duration::parse_duration;
use crate::state::ServiceState;

use super::super::{DEFAULT_HEALTHY_TIME, Event, EventKind, MAX_HEALTHY_TIME, MIN_HEALTHY_TIME};
use super::Actor;

impl Actor {
    /// Effective `healthy-time`, kept within its bounds (the plan is
    /// validated, this only guards against a bypassed validation).
    pub(super) fn healthy_time(&self) -> Duration {
        self.config
            .healthy_time
            .as_deref()
            .and_then(|s| parse_duration(s).ok())
            .unwrap_or(DEFAULT_HEALTHY_TIME)
            .clamp(MIN_HEALTHY_TIME, MAX_HEALTHY_TIME)
    }

    /// Called on every transition: an `active` period starts the
    /// healthy-time clock, any other state stops it. Oneshot jobs become
    /// healthy only by a successful run.
    pub(super) fn arm_healthy_deadline(&mut self) {
        self.healthy_deadline = (self.state == ServiceState::Active && !self.is_oneshot())
            .then(|| Instant::now() + self.healthy_time());
    }

    pub(super) async fn report_healthy(&mut self) {
        self.healthy_deadline = None;
        self.send_event(EventKind::Healthy).await;
    }

    /// `event`: `backoff`, `error` or `failed`.
    pub(super) async fn report_unhealthy(&mut self, event: &'static str, message: String) {
        self.send_event(EventKind::Unhealthy { event, message })
            .await;
    }

    /// A start (also a restart or a scheduled run) failed: the service goes
    /// to `error`.
    pub(super) async fn start_failed(&mut self, what: &str, e: &anyhow::Error) {
        error!(service = %self.name, error = %e, "{what}");
        self.transition(ServiceState::Error).await;
        self.report_unhealthy("error", format!("{what}: {e:#}"))
            .await;
    }

    async fn send_event(&self, kind: EventKind) {
        let _ = self
            .event_tx
            .send(Event {
                service: self.name.clone(),
                kind,
            })
            .await;
    }
}

/// Why a process ended, for alert messages.
pub(super) fn describe_exit(exit: Option<ExitInfo>) -> String {
    match exit {
        Some(ExitInfo {
            code: Some(code), ..
        }) => format!("exited with code {code}"),
        Some(ExitInfo {
            signal: Some(sig), ..
        }) => match nix::sys::signal::Signal::try_from(sig) {
            Ok(s) => format!("killed by {s}"),
            Err(_) => format!("killed by signal {sig}"),
        },
        _ => "ended, exit status unknown".to_owned(),
    }
}
