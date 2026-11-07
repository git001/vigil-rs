// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use prometheus_client::{
    encoding::{EncodeLabelSet, text::encode},
    metrics::{counter::Counter, family::Family, gauge::Gauge},
    registry::Registry,
};

mod check;
use check::{CheckLabel, DurationFamily, KnownChecks, LevelLabel, duration_family};

// ---------------------------------------------------------------------------
// Label types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ServiceLabel {
    service: String,
}

fn svc(service: &str) -> ServiceLabel {
    ServiceLabel {
        service: service.to_owned(),
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct RunLabel {
    service: String,
    /// `success` or `failure`.
    result: String,
}

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

/// Create a metric, register it under `name` and return the handle.
fn reg<M>(registry: &mut Registry, name: &str, help: &str) -> M
where
    M: Default + Clone + prometheus_client::registry::Metric,
{
    let metric = M::default();
    registry.register(name, help, metric.clone());
    metric
}

/// Prometheus/OpenMetrics metrics store.
///
/// Metric names and types:
/// - `vigil_service_start_count{service}` counter
/// - `vigil_service_active{service}`      gauge (0/1)
/// - `vigil_service_running{service}`     gauge (0/1, process alive: starting/active/stopping)
/// - `vigil_service_last_exit_code{service}` gauge (128+signal, -1 unknown)
/// - `vigil_service_last_run_timestamp_seconds{service}` gauge (oneshot end)
/// - `vigil_service_last_run_success{service}` gauge (oneshot result, 0/1)
/// - `vigil_service_runs{service,result}` counter (oneshot runs, result success|failure)
/// - `vigil_service_file_reloads{service}` counter (reactions to watched file changes)
/// - `vigil_check_up{check,level}`        gauge (0/1)
/// - `vigil_check_success_count{check,level}` counter
/// - `vigil_check_failure_count{check,level}` counter
/// - `vigil_check_down_count{check,level}` counter (outages: up → down)
/// - `vigil_check_down_seconds{check,level}` counter (time spent down)
/// - `vigil_health{level}`                gauge (0/1, as `GET /v1/health?level=`)
/// - `vigil_check_duration_seconds{check,level,result}` histogram (probe run)
/// - `vigil_check_connect_duration_seconds{check,level,result}` histogram (http/tcp connect)
/// - `vigil_services_count`               gauge (total number of configured services)
/// - `vigil_service_info{service}`        gauge always 1 — enumerates service names
/// - `vigil_alert_fire_count{alert}`      counter — incremented each time an alert fires
/// - `vigil_layers_reloads{result}`       counter (replan / --watch-layers, result ok|invalid)
/// - `vigil_layers_last_reload_timestamp_seconds` gauge (last successful reload)
pub struct MetricsStore {
    registry: Registry,
    service_start_count: Family<ServiceLabel, Counter>,
    service_active: Family<ServiceLabel, Gauge>,
    service_info: Family<ServiceLabel, Gauge>,
    services_count: Gauge,
    check_up: Family<CheckLabel, Gauge>,
    check_success_count: Family<CheckLabel, Counter>,
    check_failure_count: Family<CheckLabel, Counter>,
    check_down_count: Family<CheckLabel, Counter>,
    check_down_seconds: Family<CheckLabel, Counter<f64, AtomicU64>>,
    health: Family<LevelLabel, Gauge>,
    check_duration: DurationFamily,
    check_connect_duration: DurationFamily,
    known_checks: KnownChecks,
    alert_fire_count: Family<AlertLabel, Counter>,
    service_last_exit_code: Family<ServiceLabel, Gauge>,
    service_last_run_timestamp: Family<ServiceLabel, Gauge>,
    service_last_run_success: Family<ServiceLabel, Gauge>,
    service_running: Family<ServiceLabel, Gauge>,
    service_runs: Family<RunLabel, Counter>,
    service_file_reloads: Family<ServiceLabel, Counter>,
    layers_reloads: Family<ResultLabel, Counter>,
    layers_last_reload: Gauge,
}

impl MetricsStore {
    pub fn new() -> Arc<Self> {
        let mut r = Registry::default();
        Arc::new(Self {
            service_start_count: reg(
                &mut r,
                "vigil_service_start_count",
                "Number of times the service has started",
            ),
            service_active: reg(
                &mut r,
                "vigil_service_active",
                "Whether the service is currently active (1) or not (0)",
            ),
            service_info: reg(
                &mut r,
                "vigil_service_info",
                "Service metadata — label enumerates all configured service names",
            ),
            services_count: reg(
                &mut r,
                "vigil_services_count",
                "Total number of configured services",
            ),
            check_up: reg(
                &mut r,
                "vigil_check_up",
                "Whether the health check is up (1) or not (0)",
            ),
            check_success_count: reg(
                &mut r,
                "vigil_check_success_count",
                "Number of times the check has succeeded",
            ),
            check_failure_count: reg(
                &mut r,
                "vigil_check_failure_count",
                "Number of times the check has failed",
            ),
            check_down_count: reg(
                &mut r,
                "vigil_check_down_count",
                "Number of outages: the check went from up to down",
            ),
            check_down_seconds: reg(
                &mut r,
                "vigil_check_down_seconds",
                "Time the check spent down, updated at each run while it is down",
            ),
            health: reg(
                &mut r,
                "vigil_health",
                "Whether GET /v1/health?level= answers healthy (1) or not (0)",
            ),
            check_duration: {
                let family = duration_family();
                r.register(
                    "vigil_check_duration_seconds",
                    "Duration of a probe run, from its start to its result (at most the check's timeout)",
                    family.clone(),
                );
                family
            },
            check_connect_duration: {
                let family = duration_family();
                r.register(
                    "vigil_check_connect_duration_seconds",
                    "Time until the connection of a probe was up (http: DNS, TCP and TLS; tcp: the connect)",
                    family.clone(),
                );
                family
            },
            known_checks: Default::default(),
            alert_fire_count: reg(
                &mut r,
                "vigil_alert_fire_count",
                "Number of times the alert has fired (state transition detected)",
            ),
            service_last_exit_code: reg(
                &mut r,
                "vigil_service_last_exit_code",
                "Exit code of the service's last process (128+signal if killed by a signal, -1 if unknown)",
            ),
            service_last_run_timestamp: reg(
                &mut r,
                "vigil_service_last_run_timestamp_seconds",
                "Unix time when the last run of a oneshot service ended",
            ),
            service_last_run_success: reg(
                &mut r,
                "vigil_service_last_run_success",
                "Whether the last run of a oneshot service succeeded (1) or not (0)",
            ),
            service_running: reg(
                &mut r,
                "vigil_service_running",
                "Whether a process of the service is alive (starting, active or stopping)",
            ),
            service_runs: reg(
                &mut r,
                "vigil_service_runs",
                "Finished runs of a oneshot service, by result (success or failure)",
            ),
            service_file_reloads: reg(
                &mut r,
                "vigil_service_file_reloads",
                "Reactions (restart or signal) to changes of watched files",
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

    /// Set `vigil_service_info` and `vigil_services_count` to exactly these
    /// services (names removed by a replan disappear).
    pub fn set_services(&self, names: &[&str]) {
        self.service_info.clear();
        for name in names {
            self.service_info.get_or_create(&svc(name)).set(1);
        }
        self.services_count.set(names.len() as i64);
    }

    pub fn record_service_start(&self, service: &str) {
        self.service_start_count.get_or_create(&svc(service)).inc();
    }

    pub fn set_service_active(&self, service: &str, active: bool) {
        self.service_active
            .get_or_create(&svc(service))
            .set(if active { 1 } else { 0 });
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

    pub fn record_file_reload(&self, service: &str) {
        self.service_file_reloads.get_or_create(&svc(service)).inc();
    }

    pub fn set_service_running(&self, service: &str, running: bool) {
        self.service_running
            .get_or_create(&svc(service))
            .set(i64::from(running));
    }

    /// Record how the service's last process ended (`None`: unknown → -1).
    pub fn set_service_last_exit_code(&self, service: &str, code: Option<i32>) {
        self.service_last_exit_code
            .get_or_create(&svc(service))
            .set(code.map(i64::from).unwrap_or(-1));
    }

    /// Record the end of a oneshot run.
    pub fn record_oneshot_run(&self, service: &str, success: bool) {
        let label = svc(service);
        self.service_last_run_timestamp
            .get_or_create(&label)
            .set(jiff::Timestamp::now().as_second());
        self.service_last_run_success
            .get_or_create(&label)
            .set(i64::from(success));
        self.service_runs
            .get_or_create(&RunLabel {
                service: service.to_owned(),
                result: if success { "success" } else { "failure" }.to_owned(),
            })
            .inc();
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
