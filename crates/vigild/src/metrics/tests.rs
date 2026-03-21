// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

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
        "vigil_alert_fire_count",
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
    m.record_check_success("http-check");
    m.record_check_success("http-check");
    m.record_check_failure("http-check");
    let out = m.render();
    assert!(
        out.contains(r#"vigil_check_success_count_total{check="http-check"} 2"#),
        "expected success=2 in: {out}"
    );
    assert!(
        out.contains(r#"vigil_check_failure_count_total{check="http-check"} 1"#),
        "expected failure=1 in: {out}"
    );
}

#[test]
fn set_check_up_gauge_value() {
    let m = MetricsStore::new();
    m.set_check_up("probe", true);
    let out = m.render();
    assert!(
        out.contains(r#"vigil_check_up{check="probe"} 1"#),
        "expected up=1 in: {out}"
    );
    m.set_check_up("probe", false);
    let out2 = m.render();
    assert!(
        out2.contains(r#"vigil_check_up{check="probe"} 0"#),
        "expected up=0 in: {out2}"
    );
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
