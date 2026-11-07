// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! AlertSender: deduplicate alert events and queue them for delivery.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, warn};
use vigil_types::api::CheckStatus;
use vigil_types::plan::AlertConfig;

use super::delivery::{DEFAULT_DELIVERY_AGE, DEFAULT_DELIVERY_QUEUE, DeliveryJob, delivery_worker};
use super::format::format_payload;
use super::prepare::{AlertEntry, PreparedAlerts};
use super::{AlertEvent, AlertSource};

/// Routes alert events to the configured alert endpoints.
///
/// Deduplicates per `(source, name)`: a firing event is sent when it differs
/// from the last one sent, a resolved event only after a firing one.
///
/// HTTP delivery is handled by a background worker task so that the caller
/// (the Overlord event loop) is never blocked by retries or slow endpoints.
pub struct AlertSender {
    pub(super) alerts: Vec<AlertEntry>,
    /// Last known status per check name (sent or not), with the reason of
    /// a `Down`.
    observed: HashMap<String, (CheckStatus, Option<String>)>,
    /// Last firing event sent per `(source, name)`; absent = nothing open.
    open: HashMap<(AlertSource, String), String>,
    /// Sends delivery jobs to the background worker.
    pub(super) delivery_tx: mpsc::Sender<DeliveryJob>,
    /// Held until `spawn_worker()` is called; `None` afterwards.
    pub(super) delivery_rx: Option<mpsc::Receiver<DeliveryJob>>,
    /// Maximum age (seconds) a job may wait before being discarded.
    /// Shared with the delivery worker so replan takes effect immediately.
    pub(crate) max_age_secs: Arc<AtomicU64>,
    /// Capacity the channel was created with (logged in warnings).
    pub(crate) queue_depth: usize,
}

impl Default for AlertSender {
    fn default() -> Self {
        Self::new()
    }
}

impl AlertSender {
    /// Create with built-in defaults (`DEFAULT_DELIVERY_QUEUE`, `DEFAULT_DELIVERY_AGE`).
    pub fn new() -> Self {
        Self::with_queue_limits(DEFAULT_DELIVERY_QUEUE, DEFAULT_DELIVERY_AGE)
    }

    /// Create with explicit queue limits (used by the overlord when the layer
    /// config overrides the defaults via `alerts.max-queue-depth` /
    /// `alerts.max-queue-time`).
    pub fn with_queue_limits(queue_depth: usize, max_age: Duration) -> Self {
        let (delivery_tx, delivery_rx) = mpsc::channel(queue_depth);
        Self {
            alerts: Vec::new(),
            observed: HashMap::new(),
            open: HashMap::new(),
            delivery_tx,
            delivery_rx: Some(delivery_rx),
            max_age_secs: Arc::new(AtomicU64::new(max_age.as_secs())),
            queue_depth,
        }
    }

    /// Spawn the background delivery worker. Must be called once from inside
    /// a Tokio runtime (i.e. from the overlord spawn function).
    pub fn spawn_worker(&mut self) {
        if let Some(rx) = self.delivery_rx.take() {
            tokio::spawn(delivery_worker(rx, Arc::clone(&self.max_age_secs)));
        }
    }

    /// Update queue limits after a replan.
    ///
    /// - `max_age`: takes effect immediately for all queued jobs (written to the
    ///   shared atomic that the running worker reads on every job).
    /// - `queue_depth`: if changed, the existing channel is replaced and a new
    ///   worker is spawned; the old worker drains its remaining jobs and exits.
    ///   Must be called from inside a Tokio runtime.
    pub fn update_queue_limits(&mut self, queue_depth: usize, max_age: Duration) {
        self.max_age_secs
            .store(max_age.as_secs(), Ordering::Relaxed);

        if queue_depth != self.queue_depth {
            let (delivery_tx, delivery_rx) = mpsc::channel(queue_depth);
            self.delivery_tx = delivery_tx;
            self.queue_depth = queue_depth;
            tokio::spawn(delivery_worker(delivery_rx, Arc::clone(&self.max_age_secs)));
        }
    }

    /// Replace the active alerts (on plan load / replan).
    pub fn update_alerts(&mut self, alerts: PreparedAlerts) {
        self.alerts = alerts.0;
    }

    /// Return the last observed status for `check`, if any.
    pub fn check_status(&self, check: &str) -> Option<CheckStatus> {
        self.observed.get(check).map(|(status, _)| *status)
    }

    /// Record a check status and report it (on a change).
    ///
    /// Returns the names of alerts that fired (for metrics accounting).
    /// `error` is why a `Down` check failed; it goes into the message.
    pub fn handle_check_event(
        &mut self,
        check: &str,
        status: CheckStatus,
        error: Option<&str>,
    ) -> Vec<String> {
        self.observed
            .insert(check.to_owned(), (status, error.map(str::to_owned)));
        self.send(AlertEvent::check(check, status, error))
    }

    /// Only record a check status: the check belongs to a service that is
    /// not healthy. [`Self::recheck`] reports it once the service is.
    pub fn hold_check_event(&mut self, check: &str, status: CheckStatus, error: Option<&str>) {
        self.observed
            .insert(check.to_owned(), (status, error.map(str::to_owned)));
    }

    /// Report the current status of `check` (after its service became
    /// healthy): a down check that was held back fires now, one that
    /// recovered meanwhile ends an alert sent before, if any.
    pub fn recheck(&mut self, check: &str) -> Vec<String> {
        match self.observed.get(check) {
            Some((status, error)) => {
                let ev = AlertEvent::check(check, *status, error.as_deref());
                self.send(ev)
            }
            None => vec![],
        }
    }

    /// Deduplicate `ev` and enqueue it for every alert subscribed to its
    /// source and name. HTTP delivery runs in a background worker.
    ///
    /// Returns the names of alerts that fired (for metrics accounting).
    pub fn send(&mut self, ev: AlertEvent) -> Vec<String> {
        let key = (ev.source, ev.name.clone());
        if ev.firing {
            if self.open.get(&key) == Some(&ev.event) {
                return vec![]; // the same problem again
            }
            self.open.insert(key, ev.event.clone());
        } else if self.open.remove(&key).is_none() {
            return vec![]; // nothing was reported, nothing to resolve
        }

        let mut fired = Vec::new();
        for entry in &self.alerts {
            if !subscribed(&entry.config, &ev) {
                continue;
            }
            let body = format_payload(&ev, &entry.config);
            debug!(
                alert = %entry.name,
                source = ev.source.as_str(),
                name = %ev.name,
                event = %ev.event,
                "sending alert"
            );
            if self
                .delivery_tx
                .try_send(DeliveryJob {
                    client: entry.client.clone(),
                    config: entry.config.clone(),
                    body,
                    queued_at: tokio::time::Instant::now(),
                })
                .is_err()
            {
                warn!(
                    source = ev.source.as_str(),
                    name = %ev.name,
                    queue_depth = self.queue_depth,
                    "alert delivery queue full — dropping job"
                );
            } else {
                fired.push(entry.name.clone());
            }
        }
        fired
    }
}

/// Whether `cfg` subscribes to events of `ev`'s source and name.
fn subscribed(cfg: &AlertConfig, ev: &AlertEvent) -> bool {
    let names = match ev.source {
        AlertSource::Check => &cfg.on_check,
        AlertSource::Service => &cfg.on_service,
        AlertSource::Vigild => &cfg.on_vigild,
    };
    names.iter().any(|n| n == "*" || *n == ev.name)
}
