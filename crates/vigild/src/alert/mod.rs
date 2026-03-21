// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Alert routing — fires HTTP(S) notifications on check state transitions.
//!
//! Supported formats: webhook (generic JSON), Alertmanager, CloudEvents 1.0,
//! OTLP HTTP/JSON logs.
//!
//! Values in `labels` and `send_info_fields` that start with `"env:"` are
//! resolved from the process environment at send time, so you can inject
//! runtime context without rebuilding the image:
//!
//! ```yaml
//! alerts:
//!   prod:
//!     url: http://alertmanager:9093/api/v2/alerts
//!     format: alertmanager
//!     on_check: [website]
//!     labels:
//!       cluster: "env:CLUSTER_NAME"
//!     send_info_fields:
//!       k8s_service: "env:KUBERNETES_SERVICE_NAME"
//! ```

mod delivery;
mod format;
#[cfg(test)]
mod tests;

pub use self::delivery::{DEFAULT_DELIVERY_AGE, DEFAULT_DELIVERY_QUEUE};

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use indexmap::IndexMap;
use reqwest::Client;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use vigil_types::api::CheckStatus;
use vigil_types::plan::AlertConfig;

use self::delivery::{DeliveryJob, build_client, delivery_worker, warn_unset_env_vars};
use self::format::{format_payload, status_str};

// ---------------------------------------------------------------------------
// Per-alert entry (config + dedicated reqwest client for TLS options)
// ---------------------------------------------------------------------------

struct AlertEntry {
    name: String,
    config: AlertConfig,
    client: Client,
}

// ---------------------------------------------------------------------------
// AlertSender
// ---------------------------------------------------------------------------

/// Routes check state-change events to configured alert endpoints.
///
/// Deduplicates: only fires when the check status actually changes
/// (Up → Down = firing, Down → Up = resolved).
///
/// HTTP delivery is handled by a background worker task so that the caller
/// (the Overlord event loop) is never blocked by retries or slow endpoints.
pub struct AlertSender {
    alerts: Vec<AlertEntry>,
    /// Last known status per check name.
    state: HashMap<String, CheckStatus>,
    /// Sends delivery jobs to the background worker.
    delivery_tx: mpsc::Sender<DeliveryJob>,
    /// Held until `spawn_worker()` is called; `None` afterwards.
    delivery_rx: Option<mpsc::Receiver<DeliveryJob>>,
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
            state: HashMap::new(),
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

    /// Replace the active alert config (called on plan load / replan).
    pub fn update_alerts(&mut self, alerts: IndexMap<String, AlertConfig>) {
        self.alerts = alerts
            .into_iter()
            .map(|(name, cfg)| {
                warn_unset_env_vars(&name, &cfg);
                let client = build_client(&cfg);
                AlertEntry {
                    name,
                    config: cfg,
                    client,
                }
            })
            .collect();
    }

    /// Return the last observed status for `check`, if any.
    pub fn check_status(&self, check: &str) -> Option<CheckStatus> {
        self.state.get(check).copied()
    }

    /// Process a check status event. Sends only on status transitions.
    ///
    /// "Recovered" alerts (Up) are only sent if a prior Down was observed —
    /// this prevents a spurious recovery notification on the very first check.
    ///
    /// HTTP delivery is enqueued to a background worker and returns immediately.
    ///
    /// Returns the names of alerts that actually fired (for metrics accounting).
    pub fn handle_check_event(&mut self, check: &str, status: CheckStatus) -> Vec<String> {
        let prev = self.state.get(check).copied();
        if prev == Some(status) {
            return vec![]; // no change — suppress
        }
        // Suppress Up alerts that have no prior Down (first check result is Up).
        if status == CheckStatus::Up && prev.is_none() {
            self.state.insert(check.to_owned(), status);
            return vec![];
        }
        self.state.insert(check.to_owned(), status);

        let mut fired = Vec::new();
        for entry in &self.alerts {
            if !entry.config.on_check.iter().any(|c| c == check) {
                continue;
            }
            let body = format_payload(check, status, &entry.config);
            debug!(
                alert_url = %entry.config.url,
                check = %check,
                status = %status_str(status),
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
                    check = %check,
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
