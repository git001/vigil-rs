// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Alert payload formatters and field-resolution helpers.

use chrono::Utc;
use indexmap::IndexMap;
use serde_json::{Value, json};
use uuid::Uuid;
use vigil_types::api::CheckStatus;
use vigil_types::plan::{AlertConfig, AlertFormat};

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
    Value::Object(
        resolved_map(map)
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Format serialisers
// ---------------------------------------------------------------------------

pub(super) fn status_str(status: CheckStatus) -> &'static str {
    if status == CheckStatus::Down {
        "down"
    } else {
        "up"
    }
}

pub(super) fn format_payload(check: &str, status: CheckStatus, cfg: &AlertConfig) -> Value {
    match cfg.format {
        AlertFormat::Webhook => {
            if let Some(tmpl) = &cfg.body_template {
                format_webhook_template(check, status, cfg, tmpl)
            } else {
                format_webhook(check, status, cfg)
            }
        }
        AlertFormat::Alertmanager => format_alertmanager(check, status, cfg),
        AlertFormat::CloudEvents => format_cloudevents(check, status, cfg),
        AlertFormat::OtlpLogs => format_otlp_logs(check, status, cfg),
    }
}

/// Render a Jinja2-style `body_template` for the webhook format.
///
/// On template parse/render errors or invalid JSON output, a warning is logged
/// and the function falls back to the default webhook payload.
fn format_webhook_template(check: &str, status: CheckStatus, cfg: &AlertConfig, template: &str) -> Value {
    use minijinja::{Environment, context};

    let mut env = Environment::new();
    if let Err(e) = env.add_template("body", template) {
        tracing::warn!(alert_template_error = %e, "body_template parse error — falling back to default webhook payload");
        return format_webhook(check, status, cfg);
    }
    let tmpl = match env.get_template("body") {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(alert_template_error = %e, "body_template load error — falling back");
            return format_webhook(check, status, cfg);
        }
    };

    let rendered = match tmpl.render(context!(
        check     => check,
        status    => status_str(status),
        timestamp => Utc::now().to_rfc3339(),
        labels    => resolved_map(&cfg.labels),
        info      => resolved_map(&cfg.send_info_fields),
    )) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(alert_template_error = %e, "body_template render error — falling back to default webhook payload");
            return format_webhook(check, status, cfg);
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
            format_webhook(check, status, cfg)
        }
    }
}

fn format_webhook(check: &str, status: CheckStatus, cfg: &AlertConfig) -> Value {
    json!({
        "check": check,
        "status": status_str(status),
        "timestamp": Utc::now().to_rfc3339(),
        "labels": resolved_json(&cfg.labels),
        "info": resolved_json(&cfg.send_info_fields),
    })
}

fn format_alertmanager(check: &str, status: CheckStatus, cfg: &AlertConfig) -> Value {
    let now = Utc::now().to_rfc3339();
    let (starts_at, ends_at) = if status == CheckStatus::Down {
        (now, "0001-01-01T00:00:00Z".to_owned())
    } else {
        ("0001-01-01T00:00:00Z".to_owned(), Utc::now().to_rfc3339())
    };

    let mut labels = resolved_map(&cfg.labels);
    labels.insert("alertname".into(), check.to_owned());
    labels.insert("check".into(), check.to_owned());
    let labels_json: Value = Value::Object(
        labels
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect(),
    );

    json!([{
        "labels":      labels_json,
        "annotations": resolved_json(&cfg.send_info_fields),
        "startsAt":    starts_at,
        "endsAt":      ends_at,
    }])
}

fn format_cloudevents(check: &str, status: CheckStatus, cfg: &AlertConfig) -> Value {
    let event_type = if status == CheckStatus::Down {
        "io.vigil.check.failed"
    } else {
        "io.vigil.check.recovered"
    };
    json!({
        "specversion":      "1.0",
        "id":               Uuid::new_v4().to_string(),
        "source":           "vigild",
        "type":             event_type,
        "time":             Utc::now().to_rfc3339(),
        "datacontenttype":  "application/json",
        "data": {
            "check":  check,
            "status": status_str(status),
            "labels": resolved_json(&cfg.labels),
            "info":   resolved_json(&cfg.send_info_fields),
        }
    })
}

fn format_otlp_logs(check: &str, status: CheckStatus, cfg: &AlertConfig) -> Value {
    let time_ns = Utc::now().timestamp_nanos_opt().unwrap_or(0).to_string();
    let (severity_number, severity_text) = if status == CheckStatus::Down {
        (17u32, "ERROR")
    } else {
        (9u32, "INFO")
    };
    let body = if status == CheckStatus::Down {
        format!("check {} failed", check)
    } else {
        format!("check {} recovered", check)
    };

    let mut attributes: Vec<Value> = vec![
        json!({"key": "check.name",   "value": {"stringValue": check}}),
        json!({"key": "check.status", "value": {"stringValue": status_str(status)}}),
    ];
    for (k, v) in resolved_map(&cfg.labels) {
        attributes.push(json!({"key": k, "value": {"stringValue": v}}));
    }
    for (k, v) in resolved_map(&cfg.send_info_fields) {
        attributes.push(json!({"key": k, "value": {"stringValue": v}}));
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
                    "body":           {"stringValue": body},
                    "attributes":     attributes,
                }]
            }]
        }]
    })
}

#[cfg(test)]
mod tests;
