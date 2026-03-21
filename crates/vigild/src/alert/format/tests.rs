// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use vigil_types::api::CheckStatus;
use vigil_types::plan::{AlertConfig, AlertFormat};

use super::{
    format_alertmanager, format_cloudevents, format_otlp_logs, format_payload, format_webhook,
    resolve,
};

fn empty_cfg() -> AlertConfig {
    AlertConfig {
        url: "http://example.com".into(),
        ..Default::default()
    }
}

#[test]
fn resolve_literal_passthrough() {
    assert_eq!(resolve("hello"), "hello");
}

#[test]
fn resolve_env_var_set() {
    unsafe {
        std::env::set_var("_VIGIL_TEST_ALERT_VAR", "my-value");
    }
    assert_eq!(resolve("env:_VIGIL_TEST_ALERT_VAR"), "my-value");
    unsafe {
        std::env::remove_var("_VIGIL_TEST_ALERT_VAR");
    }
}

#[test]
fn resolve_env_var_unset_returns_empty() {
    unsafe {
        std::env::remove_var("_VIGIL_TEST_ALERT_MISSING");
    }
    assert_eq!(resolve("env:_VIGIL_TEST_ALERT_MISSING"), "");
}

#[test]
fn format_webhook_has_expected_keys() {
    let cfg = empty_cfg();
    let v = format_webhook("web", CheckStatus::Down, &cfg);
    assert_eq!(v["check"], "web");
    assert_eq!(v["status"], "down");
    assert!(v["timestamp"].is_string());
}

#[test]
fn format_alertmanager_down_has_zero_ends_at() {
    let cfg = empty_cfg();
    let v = format_alertmanager("web", CheckStatus::Down, &cfg);
    let alert = &v[0];
    assert_eq!(alert["endsAt"], "0001-01-01T00:00:00Z");
    assert_eq!(alert["labels"]["alertname"], "web");
}

#[test]
fn format_alertmanager_up_has_zero_starts_at() {
    let cfg = empty_cfg();
    let v = format_alertmanager("web", CheckStatus::Up, &cfg);
    let alert = &v[0];
    assert_eq!(alert["startsAt"], "0001-01-01T00:00:00Z");
}

#[test]
fn format_cloudevents_down_type() {
    let cfg = empty_cfg();
    let v = format_cloudevents("web", CheckStatus::Down, &cfg);
    assert_eq!(v["type"], "io.vigil.check.failed");
    assert_eq!(v["specversion"], "1.0");
}

#[test]
fn format_cloudevents_up_type() {
    let cfg = empty_cfg();
    let v = format_cloudevents("web", CheckStatus::Up, &cfg);
    assert_eq!(v["type"], "io.vigil.check.recovered");
}

#[test]
fn format_otlp_logs_down_severity() {
    let cfg = empty_cfg();
    let v = format_otlp_logs("web", CheckStatus::Down, &cfg);
    let record = &v["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0];
    assert_eq!(record["severityNumber"], 17);
    assert_eq!(record["severityText"], "ERROR");
}

#[test]
fn format_otlp_logs_up_severity() {
    let cfg = empty_cfg();
    let v = format_otlp_logs("web", CheckStatus::Up, &cfg);
    let record = &v["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0];
    assert_eq!(record["severityNumber"], 9);
    assert_eq!(record["severityText"], "INFO");
}

// --- body_template ---

#[test]
fn template_slack_like_down() {
    let mut cfg = empty_cfg();
    cfg.body_template = Some(r#"{"text": "Check {{ check }} is {{ status }}"}"#.to_string());
    let v = format_payload("web", CheckStatus::Down, &cfg);
    assert_eq!(v["text"], "Check web is down");
}

#[test]
fn template_slack_like_up() {
    let mut cfg = empty_cfg();
    cfg.body_template = Some(r#"{"text": "{{ check }} recovered"}"#.to_string());
    let v = format_payload("db", CheckStatus::Up, &cfg);
    assert_eq!(v["text"], "db recovered");
}

#[test]
fn template_with_labels_and_info() {
    let mut cfg = empty_cfg();
    cfg.labels.insert("env".into(), "prod".into());
    cfg.send_info_fields
        .insert("team".into(), "platform".into());
    cfg.body_template = Some(
        r#"{"env": "{{ labels.env }}", "team": "{{ info.team }}", "s": "{{ status }}"}"#
            .to_string(),
    );
    let v = format_payload("my-check", CheckStatus::Up, &cfg);
    assert_eq!(v["env"], "prod");
    assert_eq!(v["team"], "platform");
    assert_eq!(v["s"], "up");
}

#[test]
fn template_timestamp_is_string() {
    let mut cfg = empty_cfg();
    cfg.body_template = Some(r#"{"ts": "{{ timestamp }}"}"#.to_string());
    let v = format_payload("web", CheckStatus::Down, &cfg);
    assert!(
        v["ts"].as_str().unwrap().contains('T'),
        "timestamp should be RFC3339"
    );
}

#[test]
fn template_conditional_color() {
    let mut cfg = empty_cfg();
    cfg.body_template =
        Some(r#"{"color": "{% if status == 'down' %}red{% else %}green{% endif %}"}"#.to_string());
    let down = format_payload("web", CheckStatus::Down, &cfg);
    assert_eq!(down["color"], "red");
    let up = format_payload("web", CheckStatus::Up, &cfg);
    assert_eq!(up["color"], "green");
}

#[test]
fn template_invalid_jinja_falls_back_to_default() {
    let mut cfg = empty_cfg();
    cfg.body_template = Some("{{ unclosed".to_string()); // invalid syntax
    let v = format_payload("web", CheckStatus::Down, &cfg);
    // Falls back to default webhook payload
    assert_eq!(v["check"], "web");
    assert_eq!(v["status"], "down");
}

#[test]
fn template_invalid_json_output_falls_back_to_default() {
    let mut cfg = empty_cfg();
    cfg.body_template = Some("not json at all — {{ check }}".to_string());
    let v = format_payload("web", CheckStatus::Down, &cfg);
    assert_eq!(v["check"], "web");
    assert_eq!(v["status"], "down");
}

#[test]
fn template_ignored_for_alertmanager_format() {
    let mut cfg = empty_cfg();
    cfg.format = AlertFormat::Alertmanager;
    cfg.body_template = Some(r#"{"text": "should be ignored"}"#.to_string());
    let v = format_payload("web", CheckStatus::Down, &cfg);
    // Must use alertmanager format, not the template
    assert!(v.is_array());
    assert_eq!(v[0]["labels"]["alertname"], "web");
}

#[test]
fn template_ignored_for_cloudevents_format() {
    let mut cfg = empty_cfg();
    cfg.format = AlertFormat::CloudEvents;
    cfg.body_template = Some(r#"{"text": "should be ignored"}"#.to_string());
    let v = format_payload("web", CheckStatus::Down, &cfg);
    assert_eq!(v["specversion"], "1.0");
}

#[test]
fn template_ignored_for_otlp_format() {
    let mut cfg = empty_cfg();
    cfg.format = AlertFormat::OtlpLogs;
    cfg.body_template = Some(r#"{"text": "should be ignored"}"#.to_string());
    let v = format_payload("web", CheckStatus::Down, &cfg);
    assert!(v["resourceLogs"].is_array());
}
