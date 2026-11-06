// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Alert routing — fires HTTP(S) notifications for events of checks,
//! services and vigild itself (see [`AlertEvent`]).
//!
//! Supported formats: webhook (generic JSON), Alertmanager, CloudEvents 1.0,
//! OTLP HTTP/JSON logs.
//!
//! Values of `url`, `headers`, `labels`, `send-info-fields`, `proxy` and
//! `no-proxy` that start with `"env:"` come from vigild's environment, so you
//! can inject runtime context without rebuilding the image. They are
//! resolved when the layers are loaded; an unset variable makes the layer
//! invalid:
//!
//! ```yaml
//! alerts:
//!   prod:
//!     url: http://alertmanager:9093/api/v2/alerts
//!     format: alertmanager
//!     on-check: [website]
//!     on-service: ["*"]
//!     labels:
//!       cluster: "env:CLUSTER_NAME"
//!     send-info-fields:
//!       k8s_service: "env:KUBERNETES_SERVICE_NAME"
//! ```

mod delivery;
mod event;
mod format;
#[cfg(test)]
mod tests;

pub use self::delivery::{DEFAULT_DELIVERY_AGE, DEFAULT_DELIVERY_QUEUE};
pub use self::event::{AlertEvent, AlertSource};

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Context;
use indexmap::IndexMap;
use reqwest::Client;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use vigil_types::api::CheckStatus;
use vigil_types::plan::AlertConfig;

use self::delivery::{DeliveryJob, delivery_worker};
use self::format::format_payload;

// ---------------------------------------------------------------------------
// Per-alert entry (config + dedicated reqwest client for TLS options)
// ---------------------------------------------------------------------------

struct AlertEntry {
    name: String,
    /// With `env:VAR` values resolved.
    config: AlertConfig,
    client: Client,
}

/// What loading the layers prepares alerts against.
pub struct LoadEnv {
    /// vigild's environment, which each alert sees through its `pass-env*`
    /// policy.
    pub parent: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    /// Do not read the files an alert names (`tls:`): only
    /// for the shipped examples, whose paths exist in a deployment.
    #[cfg(test)]
    pub skip_files: bool,
}

impl LoadEnv {
    /// vigild's own environment (read once) and file system.
    pub fn process() -> Self {
        Self {
            parent: std::env::vars_os().collect(),
            #[cfg(test)]
            skip_files: false,
        }
    }
}

/// Alerts ready to send: `env:VAR` values resolved, HTTP clients built.
/// Made by [`prepare_alerts`] while the layers are loaded (off the async
/// runtime, as it reads the `tls:` files).
#[derive(Default)]
pub struct PreparedAlerts(Vec<AlertEntry>);

/// Prepare every alert of a plan; an error names the alert and the field
/// (an unset or not passed variable, an unreadable `tls.ca`, a bad proxy).
pub fn prepare_alerts(
    alerts: &IndexMap<String, AlertConfig>,
    env: &LoadEnv,
) -> anyhow::Result<PreparedAlerts> {
    alerts
        .iter()
        .map(|(name, cfg)| {
            let (config, client) =
                delivery::prepare(cfg, env).with_context(|| format!("alert {name:?}"))?;
            Ok(AlertEntry {
                name: name.clone(),
                config,
                client,
            })
        })
        .collect::<anyhow::Result<_>>()
        .map(PreparedAlerts)
}

// ---------------------------------------------------------------------------
// AlertSender
// ---------------------------------------------------------------------------

/// Routes alert events to the configured alert endpoints.
///
/// Deduplicates per `(source, name)`: a firing event is sent when it differs
/// from the last one sent, a resolved event only after a firing one.
///
/// HTTP delivery is handled by a background worker task so that the caller
/// (the Overlord event loop) is never blocked by retries or slow endpoints.
pub struct AlertSender {
    alerts: Vec<AlertEntry>,
    /// Last known status per check name (sent or not).
    observed: HashMap<String, CheckStatus>,
    /// Last firing event sent per `(source, name)`; absent = nothing open.
    open: HashMap<(AlertSource, String), String>,
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

/// Delivery queue limits of a plan (`alerts.max-queue-depth`,
/// `alerts.max-queue-time`), defaults where unset. The plan check rejects
/// invalid values ([`validate_queue_limits`]).
pub fn queue_limits(plan: &vigil_types::plan::Plan) -> (usize, Duration) {
    let depth = plan.alert_queue_depth.unwrap_or(DEFAULT_DELIVERY_QUEUE);
    let age = plan
        .alert_max_queue_time
        .as_deref()
        .and_then(|s| crate::duration::parse_duration(s).ok())
        .unwrap_or(DEFAULT_DELIVERY_AGE);
    (depth, age)
}

/// `max-queue-depth` must be at least 1 (a channel of 0 cannot exist) and
/// `max-queue-time` a duration of at least 1s (jobs are aged in whole
/// seconds; less would discard every alert).
pub fn validate_queue_limits(plan: &vigil_types::plan::Plan) -> anyhow::Result<()> {
    if let Some(d) = plan.alert_queue_depth {
        anyhow::ensure!(d >= 1, "alerts: max-queue-depth must be at least 1");
    }
    if let Some(t) = &plan.alert_max_queue_time {
        let d = crate::duration::parse_duration(t)
            .map_err(|e| anyhow::anyhow!("alerts: max-queue-time {t:?} is not a duration: {e}"))?;
        anyhow::ensure!(
            d >= Duration::from_secs(1),
            "alerts: max-queue-time {t:?} must be at least 1s"
        );
    }
    Ok(())
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
        self.observed.get(check).copied()
    }

    /// Record a check status and report it (on a change).
    ///
    /// Returns the names of alerts that fired (for metrics accounting).
    pub fn handle_check_event(&mut self, check: &str, status: CheckStatus) -> Vec<String> {
        self.observed.insert(check.to_owned(), status);
        self.send(AlertEvent::check(check, status))
    }

    /// Only record a check status: the check belongs to a service that is
    /// not healthy. [`Self::recheck`] reports it once the service is.
    pub fn hold_check_event(&mut self, check: &str, status: CheckStatus) {
        self.observed.insert(check.to_owned(), status);
    }

    /// Report the current status of `check` (after its service became
    /// healthy): a down check that was held back fires now, one that
    /// recovered meanwhile ends an alert sent before, if any.
    pub fn recheck(&mut self, check: &str) -> Vec<String> {
        match self.observed.get(check).copied() {
            Some(status) => self.send(AlertEvent::check(check, status)),
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
