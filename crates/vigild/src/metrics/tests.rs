// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use vigil_types::plan::CheckLevel::{Alive, Ready};

use super::MetricsStore;

#[test]
fn render_empty_registry_contains_eof_marker() {
    let m = MetricsStore::new();
    let out = m.render();
    assert!(
        out.contains("# EOF"),
        "OpenMetrics output must end with # EOF"
    );
}

#[test]
fn all_metric_family_names_present_in_empty_render() {
    let m = MetricsStore::new();
    let out = m.render();
    for name in &[
        "vigil_service_start_count",
        "vigil_service_active",
        "vigil_service_info",
        "vigil_services_count",
        "vigil_check_up",
        "vigil_check_success_count",
        "vigil_check_failure_count",
        "vigil_check_down_count",
        "vigil_check_down_seconds",
        "vigil_health",
        "vigil_alert_fire_count",
        "vigil_service_running",
        "vigil_service_runs",
        "vigil_service_last_run_success",
    ] {
        assert!(
            out.contains(name),
            "metric '{name}' missing from render output"
        );
    }
}

#[test]
fn set_services_label_and_count() {
    let m = MetricsStore::new();
    m.set_services(&["web", "db"]);
    let out = m.render();
    assert!(out.contains("web"), "service label 'web' missing");
    assert!(out.contains("db"), "service label 'db' missing");
    assert!(
        out.contains("vigil_services_count_total 2") || out.contains("vigil_services_count 2"),
        "services_count not 2 in: {out}"
    );
}

#[test]
fn record_service_start_exact_counter_value() {
    let m = MetricsStore::new();
    m.record_service_start("web");
    m.record_service_start("web");
    let out = m.render();
    // OpenMetrics encodes counters with _total suffix
    assert!(
        out.contains(r#"vigil_service_start_count_total{service="web"} 2"#),
        "expected counter=2 in: {out}"
    );
}

#[test]
fn set_service_active_gauge_value() {
    let m = MetricsStore::new();
    m.set_service_active("svc", true);
    let out = m.render();
    assert!(
        out.contains(r#"vigil_service_active{service="svc"} 1"#),
        "expected active=1 in: {out}"
    );
    m.set_service_active("svc", false);
    let out2 = m.render();
    assert!(
        out2.contains(r#"vigil_service_active{service="svc"} 0"#),
        "expected active=0 in: {out2}"
    );
}

#[test]
fn record_check_success_and_failure_exact_values() {
    let m = MetricsStore::new();
    m.set_checks(&[("http-check", Ready)]);
    m.record_check_success("http-check", Ready);
    m.record_check_success("http-check", Ready);
    m.record_check_failure("http-check", Ready);
    let out = m.render();
    assert!(
        out.contains(r#"vigil_check_success_count_total{check="http-check",level="ready"} 2"#),
        "expected success=2 in: {out}"
    );
    assert!(
        out.contains(r#"vigil_check_failure_count_total{check="http-check",level="ready"} 1"#),
        "expected failure=1 in: {out}"
    );
}

#[test]
fn set_check_up_gauge_value() {
    let m = MetricsStore::new();
    m.set_checks(&[("probe", Alive)]);
    m.set_check_up("probe", Alive, true);
    let out = m.render();
    assert!(
        out.contains(r#"vigil_check_up{check="probe",level="alive"} 1"#),
        "expected up=1 in: {out}"
    );
    m.set_check_up("probe", Alive, false);
    let out2 = m.render();
    assert!(
        out2.contains(r#"vigil_check_up{check="probe",level="alive"} 0"#),
        "expected up=0 in: {out2}"
    );
}

#[test]
fn check_outages_count_and_down_seconds() {
    let m = MetricsStore::new();
    m.set_checks(&[("db", Ready)]);
    m.record_check_down("db", Ready);
    m.record_check_down("db", Ready);
    m.add_check_down_seconds("db", Ready, 1.5);
    m.add_check_down_seconds("db", Ready, 2.0);
    let out = m.render();
    assert!(
        out.contains(r#"vigil_check_down_count_total{check="db",level="ready"} 2"#),
        "{out}"
    );
    assert!(
        out.contains(r#"vigil_check_down_seconds_total{check="db",level="ready"} 3.5"#),
        "{out}"
    );
}

#[test]
fn set_checks_drops_stale_series_and_ignores_unknown_checks() {
    let m = MetricsStore::new();
    m.set_checks(&[("web", Alive), ("db", Alive), ("cache", Ready)]);
    for (check, level) in [("web", Alive), ("db", Alive), ("cache", Ready)] {
        m.set_check_up(check, level, true);
        m.record_check_failure(check, level);
    }
    // Replan: db becomes a ready check, cache is removed.
    m.set_checks(&[("web", Alive), ("db", Ready)]);
    let out = m.render();
    assert!(
        out.contains(r#"vigil_check_up{check="web",level="alive"} 1"#),
        "{out}"
    );
    assert!(!out.contains(r#"check="db",level="alive""#), "{out}");
    assert!(!out.contains(r#"check="cache""#), "{out}");

    // A late write of the removed check or under the old level is dropped.
    m.record_check_failure("cache", Ready);
    m.set_check_up("db", Alive, false);
    m.set_check_up("db", Ready, false);
    let out = m.render();
    assert!(!out.contains(r#"check="cache""#), "{out}");
    assert!(!out.contains(r#"check="db",level="alive""#), "{out}");
    assert!(
        out.contains(r#"vigil_check_up{check="db",level="ready"} 0"#),
        "{out}"
    );
}

#[test]
fn set_health_per_level() {
    let m = MetricsStore::new();
    m.set_health(true, false);
    let out = m.render();
    assert!(out.contains(r#"vigil_health{level="alive"} 1"#), "{out}");
    assert!(out.contains(r#"vigil_health{level="ready"} 0"#), "{out}");
}

#[test]
fn record_alert_fire_exact_counter_value() {
    let m = MetricsStore::new();
    m.record_alert_fire("prod-webhook");
    m.record_alert_fire("prod-webhook");
    m.record_alert_fire("prod-webhook");
    let out = m.render();
    assert!(
        out.contains(r#"vigil_alert_fire_count_total{alert="prod-webhook"} 3"#),
        "expected alert fire count=3 in: {out}"
    );
}

#[test]
fn alert_fire_count_independent_per_alert() {
    let m = MetricsStore::new();
    m.record_alert_fire("alpha");
    m.record_alert_fire("alpha");
    m.record_alert_fire("beta");
    let out = m.render();
    assert!(
        out.contains(r#"vigil_alert_fire_count_total{alert="alpha"} 2"#),
        "alpha=2 missing in: {out}"
    );
    assert!(
        out.contains(r#"vigil_alert_fire_count_total{alert="beta"} 1"#),
        "beta=1 missing in: {out}"
    );
}

#[test]
fn set_service_running_gauge_value() {
    let m = MetricsStore::new();
    m.set_service_running("job", true);
    assert!(
        m.render()
            .contains(r#"vigil_service_running{service="job"} 1"#)
    );
    m.set_service_running("job", false);
    assert!(
        m.render()
            .contains(r#"vigil_service_running{service="job"} 0"#)
    );
}

#[test]
fn record_oneshot_run_counts_by_result() {
    let m = MetricsStore::new();
    m.record_oneshot_run("job", true);
    m.record_oneshot_run("job", false);
    m.record_oneshot_run("job", false);
    let out = m.render();
    assert!(
        out.contains(r#"vigil_service_runs_total{service="job",result="success"} 1"#),
        "expected success=1 in: {out}"
    );
    assert!(
        out.contains(r#"vigil_service_runs_total{service="job",result="failure"} 2"#),
        "expected failure=2 in: {out}"
    );
    assert!(out.contains(r#"vigil_service_last_run_success{service="job"} 0"#));
}
