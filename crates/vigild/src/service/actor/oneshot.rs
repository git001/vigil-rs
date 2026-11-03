// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `type: oneshot` and `schedule` — arming the schedule and starting runs.

use std::pin::Pin;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::time::Sleep;
use tracing::{error, info, warn};
use vigil_types::plan::{ServiceType, Startup};

use crate::service::schedule::Schedule;
use crate::state::ServiceState;

use super::{Actor, Event, EventKind};

impl Actor {
    pub(super) fn is_oneshot(&self) -> bool {
        self.config.service_type == Some(ServiceType::Oneshot)
    }

    /// (Re)compute the schedule from the config. Armed only for
    /// `startup: enabled`; reports the next run to the Overlord.
    pub(super) async fn arm_schedule(&mut self) {
        let armed = match (&self.config.schedule, self.config.startup) {
            (Some(pattern), Startup::Enabled) => {
                match Schedule::parse(pattern, self.config.schedule_tz.as_deref()) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        // Validated at load; only reachable with a vanished tzdb.
                        error!(service = %self.name, error = %e, "schedule not armed");
                        None
                    }
                }
            }
            _ => None,
        };
        self.schedule = armed;
        self.compute_next_run().await;
    }

    async fn compute_next_run(&mut self) {
        let next = self.schedule.as_ref().and_then(|s| {
            s.next_after(jiff::Timestamp::now())
                .map_err(|e| warn!(service = %self.name, error = %e, "schedule has no next run"))
                .ok()
        });
        if next != self.next_run {
            self.next_run = next;
            if let Some(n) = next {
                info!(service = %self.name, next_run = %n, "next scheduled run");
            }
            let _ = self
                .event_tx
                .send(Event {
                    service: self.name.clone(),
                    kind: EventKind::NextRun {
                        at: to_chrono(next),
                    },
                })
                .await;
        }
    }

    /// When the next run is due, as a tokio deadline. Recomputed from the
    /// wall clock on every loop iteration, so clock changes are picked up.
    pub(super) fn next_run_deadline(&self) -> Option<tokio::time::Instant> {
        let next = self.next_run?;
        let wait = next.duration_since(jiff::Timestamp::now());
        let wait = Duration::try_from(wait).unwrap_or(Duration::ZERO);
        Some(tokio::time::Instant::now() + wait)
    }

    pub(super) async fn handle_schedule_due(
        &mut self,
        stop_deadline: &mut Option<Pin<Box<Sleep>>>,
    ) {
        let Some(due) = self.next_run else { return };
        if jiff::Timestamp::now() < due {
            return; // woke early (wall clock moved back); the loop re-arms
        }
        if self.child.is_some() || self.state == ServiceState::Stopping {
            warn!(service = %self.name, "previous run still active, skipping scheduled run");
        } else {
            info!(service = %self.name, "scheduled run");
            self.transition(ServiceState::Starting).await;
            match self.do_start().await {
                Ok(()) => *stop_deadline = None,
                Err(e) => {
                    error!(service = %self.name, error = %e, "scheduled run failed to start");
                    self.transition(ServiceState::Error).await;
                }
            }
        }
        self.compute_next_run().await;
    }
}

fn to_chrono(ts: Option<jiff::Timestamp>) -> Option<DateTime<Utc>> {
    ts.and_then(|t| DateTime::from_timestamp(t.as_second(), t.subsec_nanosecond() as u32))
}
