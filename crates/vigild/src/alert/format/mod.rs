// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Alert payload formatters and field-resolution helpers.

use chrono::Utc;
use indexmap::IndexMap;
use serde_json::{Value, json};
use uuid::Uuid;
use vigil_types::plan::{AlertConfig, AlertFormat};

use super::event::{AlertEvent, AlertSource};

// ---------------------------------------------------------------------------
// Field resolution
// ---------------------------------------------------------------------------

pub(crate) fn resolve(val: &str) -> String {
    if let Some(var) = val.strip_prefix("env:") {
        std::env::var(var).unwrap_or_default()
    } else {
        val.to_owned()
    }
}

pub(super) fn resolved_map(map: &IndexMap<String, String>) -> IndexMap<String, String> {
    map.iter().map(|(k, v)| (k.clone(), resolve(v))).collect()
}

fn resolved_json(map: &IndexMap<String, String>) -> Value {
    to_json(resolved_map(map))
}

fn to_json(map: IndexMap<String, String>) -> Value {
    Value::Object(
        map.into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect(),
    )
}

/// The fields of the webhook payload and of CloudEvents `data`.
fn event_json(ev: &AlertEvent, cfg: &AlertConfig) -> Value {
    let mut v = json!({
        "source":  ev.source.as_str(),
        "name":    ev.name,
        "event":   ev.event,
        "status":  ev.status_str(),
        "message": ev.message,
        "labels":  resolved_json(&cfg.labels),
        "info":    resolved_json(&cfg.send_info_fields),
    });
    if let Some(check) = check_name(ev) {
        v["check"] = json!(check);
    }
    v
}

// ---------------------------------------------------------------------------
// Format serialisers
// ---------------------------------------------------------------------------

pub(super) fn format_payload(ev: &AlertEvent, cfg: &AlertConfig) -> Value {
    match cfg.format() {
        AlertFormat::Webhook => {
            if let Some(tmpl) = &cfg.body_template {
                format_webhook_template(ev, cfg, tmpl)
            } else {
                format_webhook(ev, cfg)
            }
        }
        AlertFormat::Alertmanager => format_alertmanager(ev, cfg),
        AlertFormat::CloudEvents => format_cloudevents(ev, cfg),
        AlertFormat::OtlpLogs => format_otlp_logs(ev, cfg),
    }
}

/// `check` field of check events (kept from the check-only payloads).
fn check_name(ev: &AlertEvent) -> Option<&str> {
    (ev.source == AlertSource::Check).then_some(ev.name.as_str())
}

/// Render a Jinja2-style `body_template` for the webhook format.
///
/// On template parse/render errors or invalid JSON output, a warning is logged
/// and the function falls back to the default webhook payload.
fn format_webhook_template(ev: &AlertEvent, cfg: &AlertConfig, template: &str) -> Value {
    use minijinja::{Environment, context};

    // The syntax is checked when the layers load; this only fails on
    // runtime errors (e.g. calling an unknown function).
    let rendered = match Environment::new().render_str(
        template,
        context!(
            // Undefined (renders empty) for events that are not about a check.
            check     => check_name(ev).map_or(minijinja::Value::UNDEFINED, minijinja::Value::from),
            source    => ev.source.as_str(),
            name      => &ev.name,
            event     => &ev.event,
            status    => ev.status_str(),
            message   => &ev.message,
            timestamp => Utc::now().to_rfc3339(),
            labels    => resolved_map(&cfg.labels),
            info      => resolved_map(&cfg.send_info_fields),
        ),
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(alert_template_error = %e, "body_template render error — falling back to default webhook payload");
            return format_webhook(ev, cfg);
        }
    };

    match serde_json::from_str::<Value>(&rendered) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                alert_template_error = %e,
                rendered_output = %rendered,
                "body_template rendered output is not valid JSON — falling back to default webhook payload"
            );
            format_webhook(ev, cfg)
        }
    }
}

fn format_webhook(ev: &AlertEvent, cfg: &AlertConfig) -> Value {
    let mut v = event_json(ev, cfg);
    v["timestamp"] = json!(Utc::now().to_rfc3339());
    v
}

/// Alertmanager identifies an alert by its labels, so `event` (which
/// differs between the firing and the resolved event) and `message` are
/// annotations; otherwise a resolve would not end the alert it belongs to.
fn format_alertmanager(ev: &AlertEvent, cfg: &AlertConfig) -> Value {
    let now = Utc::now().to_rfc3339();
    let (starts_at, ends_at) = if ev.firing {
        (now, "0001-01-01T00:00:00Z".to_owned())
    } else {
        ("0001-01-01T00:00:00Z".to_owned(), now)
    };

    let mut labels = resolved_map(&cfg.labels);
    labels.insert("alertname".into(), ev.name.clone());
    labels.insert("source".into(), ev.source.as_str().to_owned());
    if let Some(check) = check_name(ev) {
        labels.insert("check".into(), check.to_owned());
    }
    let mut annotations = IndexMap::new();
    annotations.insert("event".to_owned(), ev.event.clone());
    annotations.insert("message".to_owned(), ev.message.clone());
    annotations.extend(resolved_map(&cfg.send_info_fields));

    json!([{
        "labels":      to_json(labels),
        "annotations": to_json(annotations),
        "startsAt":    starts_at,
        "endsAt":      ends_at,
    }])
}

/// Type `io.vigil.<source>.<event>`; check events keep
/// `io.vigil.check.failed` / `io.vigil.check.recovered`.
fn format_cloudevents(ev: &AlertEvent, cfg: &AlertConfig) -> Value {
    let data = event_json(ev, cfg);
    json!({
        "specversion":      "1.0",
        "id":               Uuid::new_v4().to_string(),
        "source":           "vigild",
        "type":             format!("io.vigil.{}.{}", ev.source.as_str(), ev.event),
        "time":             Utc::now().to_rfc3339(),
        "datacontenttype":  "application/json",
        "data":             data,
    })
}

fn format_otlp_logs(ev: &AlertEvent, cfg: &AlertConfig) -> Value {
    let time_ns = Utc::now().timestamp_nanos_opt().unwrap_or(0).to_string();
    let (severity_number, severity_text) = if ev.firing {
        (17u32, "ERROR")
    } else {
        (9u32, "INFO")
    };

    let attr = |k: &str, v: &str| json!({"key": k, "value": {"stringValue": v}});
    let mut attributes: Vec<Value> = Vec::new();
    if let Some(check) = check_name(ev) {
        attributes.push(attr("check.name", check));
        attributes.push(attr("check.status", ev.status_str()));
    }
    attributes.push(attr("vigil.source", ev.source.as_str()));
    attributes.push(attr("vigil.name", &ev.name));
    attributes.push(attr("vigil.event", &ev.event));
    for (k, v) in resolved_map(&cfg.labels) {
        attributes.push(attr(&k, &v));
    }
    for (k, v) in resolved_map(&cfg.send_info_fields) {
        attributes.push(attr(&k, &v));
    }

    json!({
        "resourceLogs": [{
            "resource": {"attributes": []},
            "scopeLogs": [{
                "scope": {
                    "name":    "vigild",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "logRecords": [{
                    "timeUnixNano":   time_ns,
                    "severityNumber": severity_number,
                    "severityText":   severity_text,
                    "body":           {"stringValue": ev.message},
                    "attributes":     attributes,
                }]
            }]
        }]
    })
}

#[cfg(test)]
mod tests;
