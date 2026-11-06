// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use vigil_types::api::CheckStatus;
use vigil_types::plan::{AlertConfig, AlertFormat};

use super::super::event::{AlertEvent, AlertSource};
use super::{
    format_alertmanager, format_cloudevents, format_otlp_logs, format_payload, format_webhook,
};

fn empty_cfg() -> AlertConfig {
    AlertConfig {
        url: "http://example.com".into(),
        ..Default::default()
    }
}

#[test]
fn format_webhook_has_expected_keys() {
    let cfg = empty_cfg();
    let v = format_webhook(&AlertEvent::check("web", CheckStatus::Down), &cfg);
    assert_eq!(v["check"], "web");
    assert_eq!(v["status"], "down");
    assert!(v["timestamp"].is_string());
}

#[test]
fn format_alertmanager_down_has_zero_ends_at() {
    let cfg = empty_cfg();
    let v = format_alertmanager(&AlertEvent::check("web", CheckStatus::Down), &cfg);
    let alert = &v[0];
    assert_eq!(alert["endsAt"], "0001-01-01T00:00:00Z");
    assert_eq!(alert["labels"]["alertname"], "web");
}

#[test]
fn format_alertmanager_up_has_zero_starts_at() {
    let cfg = empty_cfg();
    let v = format_alertmanager(&AlertEvent::check("web", CheckStatus::Up), &cfg);
    let alert = &v[0];
    assert_eq!(alert["startsAt"], "0001-01-01T00:00:00Z");
}

#[test]
fn format_cloudevents_down_type() {
    let cfg = empty_cfg();
    let v = format_cloudevents(&AlertEvent::check("web", CheckStatus::Down), &cfg);
    assert_eq!(v["type"], "io.vigil.check.failed");
    assert_eq!(v["specversion"], "1.0");
}

#[test]
fn format_cloudevents_up_type() {
    let cfg = empty_cfg();
    let v = format_cloudevents(&AlertEvent::check("web", CheckStatus::Up), &cfg);
    assert_eq!(v["type"], "io.vigil.check.recovered");
}

#[test]
fn format_otlp_logs_down_severity() {
    let cfg = empty_cfg();
    let v = format_otlp_logs(&AlertEvent::check("web", CheckStatus::Down), &cfg);
    let record = &v["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0];
    assert_eq!(record["severityNumber"], 17);
    assert_eq!(record["severityText"], "ERROR");
}

#[test]
fn format_otlp_logs_up_severity() {
    let cfg = empty_cfg();
    let v = format_otlp_logs(&AlertEvent::check("web", CheckStatus::Up), &cfg);
    let record = &v["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0];
    assert_eq!(record["severityNumber"], 9);
    assert_eq!(record["severityText"], "INFO");
}

// --- body_template ---

#[test]
fn template_slack_like_down() {
    let mut cfg = empty_cfg();
    cfg.body_template = Some(r#"{"text": "Check {{ check }} is {{ status }}"}"#.to_string());
    let v = format_payload(&AlertEvent::check("web", CheckStatus::Down), &cfg);
    assert_eq!(v["text"], "Check web is down");
}

#[test]
fn template_slack_like_up() {
    let mut cfg = empty_cfg();
    cfg.body_template = Some(r#"{"text": "{{ check }} recovered"}"#.to_string());
    let v = format_payload(&AlertEvent::check("db", CheckStatus::Up), &cfg);
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
    let v = format_payload(&AlertEvent::check("my-check", CheckStatus::Up), &cfg);
    assert_eq!(v["env"], "prod");
    assert_eq!(v["team"], "platform");
    assert_eq!(v["s"], "up");
}

#[test]
fn template_timestamp_is_string() {
    let mut cfg = empty_cfg();
    cfg.body_template = Some(r#"{"ts": "{{ timestamp }}"}"#.to_string());
    let v = format_payload(&AlertEvent::check("web", CheckStatus::Down), &cfg);
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
    let down = format_payload(&AlertEvent::check("web", CheckStatus::Down), &cfg);
    assert_eq!(down["color"], "red");
    let up = format_payload(&AlertEvent::check("web", CheckStatus::Up), &cfg);
    assert_eq!(up["color"], "green");
}

#[test]
fn template_invalid_jinja_falls_back_to_default() {
    let mut cfg = empty_cfg();
    cfg.body_template = Some("{{ unclosed".to_string()); // invalid syntax
    let v = format_payload(&AlertEvent::check("web", CheckStatus::Down), &cfg);
    // Falls back to default webhook payload
    assert_eq!(v["check"], "web");
    assert_eq!(v["status"], "down");
}

#[test]
fn template_invalid_json_output_falls_back_to_default() {
    let mut cfg = empty_cfg();
    cfg.body_template = Some("not json at all — {{ check }}".to_string());
    let v = format_payload(&AlertEvent::check("web", CheckStatus::Down), &cfg);
    assert_eq!(v["check"], "web");
    assert_eq!(v["status"], "down");
}

#[test]
fn template_ignored_for_alertmanager_format() {
    let mut cfg = empty_cfg();
    cfg.format = Some(AlertFormat::Alertmanager);
    cfg.body_template = Some(r#"{"text": "should be ignored"}"#.to_string());
    let v = format_payload(&AlertEvent::check("web", CheckStatus::Down), &cfg);
    // Must use alertmanager format, not the template
    assert!(v.is_array());
    assert_eq!(v[0]["labels"]["alertname"], "web");
}

#[test]
fn template_ignored_for_cloudevents_format() {
    let mut cfg = empty_cfg();
    cfg.format = Some(AlertFormat::CloudEvents);
    cfg.body_template = Some(r#"{"text": "should be ignored"}"#.to_string());
    let v = format_payload(&AlertEvent::check("web", CheckStatus::Down), &cfg);
    assert_eq!(v["specversion"], "1.0");
}

#[test]
fn template_ignored_for_otlp_format() {
    let mut cfg = empty_cfg();
    cfg.format = Some(AlertFormat::OtlpLogs);
    cfg.body_template = Some(r#"{"text": "should be ignored"}"#.to_string());
    let v = format_payload(&AlertEvent::check("web", CheckStatus::Down), &cfg);
    assert!(v["resourceLogs"].is_array());
}

// --- events of services and vigild ---

fn backoff() -> AlertEvent {
    AlertEvent::firing(
        AlertSource::Service,
        "app",
        "backoff",
        "exited with code 3, restarting in 2s",
    )
}

#[test]
fn webhook_service_event_has_no_check_field() {
    let v = format_webhook(&backoff(), &empty_cfg());
    assert_eq!(v["source"], "service");
    assert_eq!(v["name"], "app");
    assert_eq!(v["event"], "backoff");
    assert_eq!(v["status"], "down");
    assert_eq!(v["message"], "exited with code 3, restarting in 2s");
    assert!(v.get("check").is_none(), "{v}");
}

#[test]
fn webhook_check_event_keeps_check_field() {
    let v = format_webhook(&AlertEvent::check("web", CheckStatus::Up), &empty_cfg());
    assert_eq!(v["check"], "web");
    assert_eq!(v["source"], "check");
    assert_eq!(v["event"], "recovered");
    assert_eq!(v["status"], "up");
}

#[test]
fn alertmanager_resolve_has_the_labels_of_the_firing_event() {
    let cfg = empty_cfg();
    let firing = format_alertmanager(&backoff(), &cfg);
    let resolved = format_alertmanager(
        &AlertEvent::resolved(AlertSource::Service, "app", "healthy again"),
        &cfg,
    );
    assert_eq!(firing[0]["labels"], resolved[0]["labels"]);
    assert_eq!(firing[0]["labels"]["alertname"], "app");
    assert_eq!(firing[0]["labels"]["source"], "service");
    assert_eq!(firing[0]["annotations"]["event"], "backoff");
    assert_eq!(resolved[0]["annotations"]["event"], "recovered");
}

#[test]
fn cloudevents_type_names_source_and_event() {
    let v = format_cloudevents(&backoff(), &empty_cfg());
    assert_eq!(v["type"], "io.vigil.service.backoff");
    assert_eq!(v["data"]["message"], "exited with code 3, restarting in 2s");
    let layers = AlertEvent::firing(AlertSource::Vigild, "layers", "invalid", "bad yaml");
    let v = format_cloudevents(&layers, &empty_cfg());
    assert_eq!(v["type"], "io.vigil.vigild.invalid");
}

#[test]
fn otlp_body_is_the_message() {
    let v = format_otlp_logs(&backoff(), &empty_cfg());
    let record = &v["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0];
    assert_eq!(
        record["body"]["stringValue"],
        "exited with code 3, restarting in 2s"
    );
    assert_eq!(record["severityText"], "ERROR");
    let attrs = record["attributes"].as_array().unwrap();
    assert!(
        attrs
            .iter()
            .any(|a| a["key"] == "vigil.event" && a["value"]["stringValue"] == "backoff")
    );
    assert!(!attrs.iter().any(|a| a["key"] == "check.name"));
}

#[test]
fn template_sees_source_name_event_and_message() {
    let mut cfg = empty_cfg();
    cfg.body_template = Some(
        r#"{"text": "{{ source }} {{ name }}: {{ event }} ({{ message }})", "check": "{{ check }}"}"#
            .to_string(),
    );
    let v = format_payload(&backoff(), &cfg);
    assert_eq!(
        v["text"],
        "service app: backoff (exited with code 3, restarting in 2s)"
    );
    assert_eq!(v["check"], "", "check is empty for non-check events");
}

#[test]
fn otlp_attributes_carry_labels_and_send_info_fields() {
    let mut cfg = empty_cfg();
    cfg.labels.insert("cluster".into(), "prod-eu-1".into());
    cfg.send_info_fields
        .insert("pod".into(), "api-7d9f-x2x4q".into());
    let v = format_otlp_logs(&backoff(), &cfg);
    let attrs = v["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0]["attributes"]
        .as_array()
        .unwrap()
        .clone();
    let get = |k: &str| {
        attrs
            .iter()
            .find(|a| a["key"] == k)
            .map(|a| a["value"]["stringValue"].clone())
    };
    assert_eq!(get("cluster"), Some("prod-eu-1".into()));
    assert_eq!(get("pod"), Some("api-7d9f-x2x4q".into()));
}
