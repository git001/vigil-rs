// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;

use prometheus_client::{
    encoding::{EncodeLabelSet, text::encode},
    metrics::{counter::Counter, family::Family, gauge::Gauge},
    registry::Registry,
};

// ---------------------------------------------------------------------------
// Label types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ServiceLabel {
    service: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct CheckLabel {
    check: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct AlertLabel {
    alert: String,
}

// ---------------------------------------------------------------------------
// MetricsStore
// ---------------------------------------------------------------------------

/// Prometheus/OpenMetrics metrics store.
///
/// Compatible with Pebble's metric names and types:
/// - `vigil_service_start_count{service}` counter
/// - `vigil_service_active{service}`      gauge (0/1)
/// - `vigil_check_up{check}`              gauge (0/1)
/// - `vigil_check_success_count{check}`   counter
/// - `vigil_check_failure_count{check}`   counter
/// - `vigil_services_count`               gauge (total number of configured services)
/// - `vigil_service_info{service}`        gauge always 1 — enumerates service names
/// - `vigil_alert_fire_count{alert}`      counter — incremented each time an alert fires
pub struct MetricsStore {
    registry: Registry,
    service_start_count: Family<ServiceLabel, Counter>,
    service_active: Family<ServiceLabel, Gauge>,
    service_info: Family<ServiceLabel, Gauge>,
    services_count: Gauge,
    check_up: Family<CheckLabel, Gauge>,
    check_success_count: Family<CheckLabel, Counter>,
    check_failure_count: Family<CheckLabel, Counter>,
    alert_fire_count: Family<AlertLabel, Counter>,
}

impl MetricsStore {
    pub fn new() -> Arc<Self> {
        let mut registry = Registry::default();

        let service_start_count = Family::<ServiceLabel, Counter>::default();
        let service_active = Family::<ServiceLabel, Gauge>::default();
        let service_info = Family::<ServiceLabel, Gauge>::default();
        let services_count: Gauge = Gauge::default();
        let check_up = Family::<CheckLabel, Gauge>::default();
        let check_success_count = Family::<CheckLabel, Counter>::default();
        let check_failure_count = Family::<CheckLabel, Counter>::default();
        let alert_fire_count = Family::<AlertLabel, Counter>::default();

        registry.register(
            "vigil_service_start_count",
            "Number of times the service has started",
            service_start_count.clone(),
        );
        registry.register(
            "vigil_service_active",
            "Whether the service is currently active (1) or not (0)",
            service_active.clone(),
        );
        registry.register(
            "vigil_service_info",
            "Service metadata — label enumerates all configured service names",
            service_info.clone(),
        );
        registry.register(
            "vigil_services_count",
            "Total number of configured services",
            services_count.clone(),
        );
        registry.register(
            "vigil_check_up",
            "Whether the health check is up (1) or not (0)",
            check_up.clone(),
        );
        registry.register(
            "vigil_check_success_count",
            "Number of times the check has succeeded",
            check_success_count.clone(),
        );
        registry.register(
            "vigil_check_failure_count",
            "Number of times the check has failed",
            check_failure_count.clone(),
        );
        registry.register(
            "vigil_alert_fire_count",
            "Number of times the alert has fired (state transition detected)",
            alert_fire_count.clone(),
        );

        Arc::new(Self {
            registry,
            service_start_count,
            service_active,
            service_info,
            services_count,
            check_up,
            check_success_count,
            check_failure_count,
            alert_fire_count,
        })
    }

    /// Register a service name in `vigil_service_info` and update the count.
    pub fn set_services(&self, names: &[&str]) {
        for name in names {
            self.service_info
                .get_or_create(&ServiceLabel {
                    service: name.to_string(),
                })
                .set(1);
        }
        self.services_count.set(names.len() as i64);
    }

    pub fn record_service_start(&self, service: &str) {
        self.service_start_count
            .get_or_create(&ServiceLabel {
                service: service.to_owned(),
            })
            .inc();
    }

    pub fn set_service_active(&self, service: &str, active: bool) {
        self.service_active
            .get_or_create(&ServiceLabel {
                service: service.to_owned(),
            })
            .set(if active { 1 } else { 0 });
    }

    pub fn record_check_success(&self, check: &str) {
        self.check_success_count
            .get_or_create(&CheckLabel {
                check: check.to_owned(),
            })
            .inc();
    }

    pub fn record_check_failure(&self, check: &str) {
        self.check_failure_count
            .get_or_create(&CheckLabel {
                check: check.to_owned(),
            })
            .inc();
    }

    pub fn set_check_up(&self, check: &str, up: bool) {
        self.check_up
            .get_or_create(&CheckLabel {
                check: check.to_owned(),
            })
            .set(if up { 1 } else { 0 });
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
