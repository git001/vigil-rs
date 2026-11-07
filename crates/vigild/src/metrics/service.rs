// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Service metrics: every series has the label `service`.

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::{counter::Counter, family::Family, gauge::Gauge};
use prometheus_client::registry::Registry;

use super::{MetricsStore, reg};

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

pub(super) struct ServiceMetrics {
    start_count: Family<ServiceLabel, Counter>,
    active: Family<ServiceLabel, Gauge>,
    info: Family<ServiceLabel, Gauge>,
    count: Gauge,
    last_exit_code: Family<ServiceLabel, Gauge>,
    last_run_timestamp: Family<ServiceLabel, Gauge>,
    last_run_success: Family<ServiceLabel, Gauge>,
    running: Family<ServiceLabel, Gauge>,
    runs: Family<RunLabel, Counter>,
    file_reloads: Family<ServiceLabel, Counter>,
}

impl ServiceMetrics {
    pub(super) fn register(r: &mut Registry) -> Self {
        Self {
            start_count: reg(
                r,
                "vigil_service_start_count",
                "Number of times the service has started",
            ),
            active: reg(
                r,
                "vigil_service_active",
                "Whether the service is currently active (1) or not (0)",
            ),
            info: reg(
                r,
                "vigil_service_info",
                "Service metadata — label enumerates all configured service names",
            ),
            count: reg(
                r,
                "vigil_services_count",
                "Total number of configured services",
            ),
            last_exit_code: reg(
                r,
                "vigil_service_last_exit_code",
                "Exit code of the service's last process (128+signal if killed by a signal, -1 if unknown)",
            ),
            last_run_timestamp: reg(
                r,
                "vigil_service_last_run_timestamp_seconds",
                "Unix time when the last run of a oneshot service ended",
            ),
            last_run_success: reg(
                r,
                "vigil_service_last_run_success",
                "Whether the last run of a oneshot service succeeded (1) or not (0)",
            ),
            running: reg(
                r,
                "vigil_service_running",
                "Whether a process of the service is alive (starting, active or stopping)",
            ),
            runs: reg(
                r,
                "vigil_service_runs",
                "Finished runs of a oneshot service, by result (success or failure)",
            ),
            file_reloads: reg(
                r,
                "vigil_service_file_reloads",
                "Reactions (restart or signal) to changes of watched files",
            ),
        }
    }
}

impl MetricsStore {
    /// Set `vigil_service_info` and `vigil_services_count` to exactly these
    /// services (names removed by a replan disappear).
    pub fn set_services(&self, names: &[&str]) {
        self.services.info.clear();
        for name in names {
            self.services.info.get_or_create(&svc(name)).set(1);
        }
        self.services.count.set(names.len() as i64);
    }

    pub fn record_service_start(&self, service: &str) {
        self.services.start_count.get_or_create(&svc(service)).inc();
    }

    pub fn set_service_active(&self, service: &str, active: bool) {
        self.services
            .active
            .get_or_create(&svc(service))
            .set(i64::from(active));
    }

    pub fn record_file_reload(&self, service: &str) {
        self.services
            .file_reloads
            .get_or_create(&svc(service))
            .inc();
    }

    pub fn set_service_running(&self, service: &str, running: bool) {
        self.services
            .running
            .get_or_create(&svc(service))
            .set(i64::from(running));
    }

    /// Record how the service's last process ended (`None`: unknown → -1).
    pub fn set_service_last_exit_code(&self, service: &str, code: Option<i32>) {
        self.services
            .last_exit_code
            .get_or_create(&svc(service))
            .set(code.map(i64::from).unwrap_or(-1));
    }

    /// Record the end of a oneshot run.
    pub fn record_oneshot_run(&self, service: &str, success: bool) {
        let label = svc(service);
        self.services
            .last_run_timestamp
            .get_or_create(&label)
            .set(jiff::Timestamp::now().as_second());
        self.services
            .last_run_success
            .get_or_create(&label)
            .set(i64::from(success));
        self.services
            .runs
            .get_or_create(&RunLabel {
                service: service.to_owned(),
                result: if success { "success" } else { "failure" }.to_owned(),
            })
            .inc();
    }
}
