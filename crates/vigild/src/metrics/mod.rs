// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;

use prometheus_client::{
    encoding::{EncodeLabelSet, text::encode},
    metrics::{counter::Counter, family::Family, gauge::Gauge},
    registry::Registry,
};

mod check;
mod service;

// ---------------------------------------------------------------------------
// Label types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ResultLabel {
    /// `ok` or `invalid`.
    result: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct AlertLabel {
    alert: String,
}

// ---------------------------------------------------------------------------
// MetricsStore
// ---------------------------------------------------------------------------

/// Register `metric` under `name` and return the handle.
fn register<M>(registry: &mut Registry, name: &str, help: &str, metric: M) -> M
where
    M: Clone + prometheus_client::registry::Metric,
{
    registry.register(name, help, metric.clone());
    metric
}

/// Create a metric, register it under `name` and return the handle.
fn reg<M>(registry: &mut Registry, name: &str, help: &str) -> M
where
    M: Default + Clone + prometheus_client::registry::Metric,
{
    register(registry, name, help, M::default())
}

/// Prometheus/OpenMetrics metrics store. The metrics are listed in
/// docs/operator-guide/metrics.md.
pub struct MetricsStore {
    registry: Registry,
    services: service::ServiceMetrics,
    checks: check::CheckMetrics,
    alert_fire_count: Family<AlertLabel, Counter>,
    layers_reloads: Family<ResultLabel, Counter>,
    layers_last_reload: Gauge,
}

impl MetricsStore {
    pub fn new() -> Arc<Self> {
        let mut r = Registry::default();
        Arc::new(Self {
            services: service::ServiceMetrics::register(&mut r),
            checks: check::CheckMetrics::register(&mut r),
            alert_fire_count: reg(
                &mut r,
                "vigil_alert_fire_count",
                "Number of times the alert has fired (state transition detected)",
            ),
            layers_reloads: reg(
                &mut r,
                "vigil_layers_reloads",
                "Layer reloads (vigil replan or --watch-layers), by result (ok or invalid)",
            ),
            layers_last_reload: reg(
                &mut r,
                "vigil_layers_last_reload_timestamp_seconds",
                "Unix time of the last successful layer reload (0 = none since start)",
            ),
            registry: r,
        })
    }

    /// A layer reload finished: `ok` (applied) or not (invalid layers).
    pub fn record_layers_reload(&self, ok: bool) {
        self.layers_reloads
            .get_or_create(&ResultLabel {
                result: if ok { "ok" } else { "invalid" }.to_owned(),
            })
            .inc();
        if ok {
            self.layers_last_reload.set(chrono::Utc::now().timestamp());
        }
    }

    /// Increment the alert fire counter for the named alert.
    pub fn record_alert_fire(&self, alert: &str) {
        self.alert_fire_count
            .get_or_create(&AlertLabel {
                alert: alert.to_owned(),
            })
            .inc();
    }

    /// Render all metrics in OpenMetrics text exposition format.
    pub fn render(&self) -> String {
        let mut buf = String::new();
        encode(&mut buf, &self.registry).expect("metrics encode error");
        buf
    }
}

#[cfg(test)]
mod tests;
