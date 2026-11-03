// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Readiness — `ready: process | notify | check`, sd_notify messages and
//! the ready timeout.

use std::pin::Pin;
use std::time::Duration;

use tokio::time::Sleep;
use tracing::{debug, info, warn};
use vigil_types::plan::ReadyMode;

use crate::duration::parse_duration;
use crate::service::DEFAULT_READY_TIMEOUT;
use crate::service::notify::NotifyMsg;
use crate::state::ServiceState;

use super::{Actor, Event, EventKind};

impl Actor {
    pub(super) fn ready_mode(&self) -> ReadyMode {
        self.config.ready.unwrap_or_default()
    }

    /// `None` = wait forever (`ready-timeout: 0s`).
    pub(super) fn ready_timeout(&self) -> Option<Duration> {
        let d = self
            .config
            .ready_timeout
            .as_deref()
            .and_then(|s| parse_duration(s).ok())
            .unwrap_or(DEFAULT_READY_TIMEOUT);
        (!d.is_zero()).then_some(d)
    }

    /// Called right after a successful spawn (state is `Starting`).
    pub(super) async fn after_spawn(&mut self) {
        self.ready_failed = false;
        if self.status_message.take().is_some() {
            self.send_status_message().await;
        }
        match self.ready_mode() {
            ReadyMode::Process => self.transition(ServiceState::Active).await,
            mode => {
                self.ready_deadline = self
                    .ready_timeout()
                    .map(|d| tokio::time::Instant::now() + d);
                info!(service = %self.name, ?mode, timeout = ?self.ready_timeout(), "waiting for readiness");
            }
        }
    }

    async fn mark_ready(&mut self, how: &str) {
        if self.state != ServiceState::Starting || self.ready_failed {
            return;
        }
        info!(service = %self.name, how, "service is ready");
        self.ready_deadline = None;
        self.transition(ServiceState::Active).await;
    }

    pub(super) async fn handle_check_passed(
        &mut self,
        check: &str,
        started_at: std::time::Instant,
    ) {
        if self.ready_mode() != ReadyMode::Check
            || self.config.ready_check.as_deref() != Some(check)
            || self.child.is_none()
        {
            return;
        }
        // A probe that began before this process was spawned says nothing
        // about it.
        if started_at < self.spawned_at {
            debug!(service = %self.name, check, "ignoring probe started before spawn");
            return;
        }
        self.mark_ready("check passed").await;
    }

    pub(super) async fn handle_notify(&mut self, msg: NotifyMsg) {
        if msg.generation != self.notify_generation {
            debug!(service = %self.name, "notify message for an earlier start ignored");
            return;
        }
        let Some(main_pid) = self.child.as_ref().and_then(|c| c.id()) else {
            return;
        };
        if !msg.is_from(main_pid as i32, self.service_uid) {
            warn!(
                service = %self.name,
                sender_pid = msg.pid,
                sender_uid = msg.uid,
                "notify message from a process outside the service rejected"
            );
            return;
        }
        if let Some(status) = msg.get("STATUS") {
            self.status_message = Some(status.to_string());
            self.send_status_message().await;
        }
        if msg.get("STOPPING") == Some("1") {
            info!(service = %self.name, "service reports it is stopping");
        }
        if msg.get("READY") == Some("1") {
            self.mark_ready("READY=1").await;
        }
    }

    async fn send_status_message(&self) {
        let _ = self
            .event_tx
            .send(Event {
                service: self.name.clone(),
                kind: EventKind::StatusMessage {
                    message: self.status_message.clone(),
                },
            })
            .await;
    }

    /// Readiness was not reached in time: stop the process; its exit counts
    /// as a failure (`on-failure` applies).
    pub(super) async fn handle_ready_timeout(
        &mut self,
        stop_deadline: &mut Option<Pin<Box<Sleep>>>,
    ) {
        self.ready_deadline = None;
        if self.state != ServiceState::Starting || self.child.is_none() {
            return;
        }
        warn!(
            service = %self.name,
            timeout = ?self.ready_timeout(),
            "service did not become ready in time, stopping it"
        );
        self.ready_failed = true;
        self.send_stop_signal();
        *stop_deadline = Some(Box::pin(tokio::time::sleep(self.kill_delay())));
    }
}
