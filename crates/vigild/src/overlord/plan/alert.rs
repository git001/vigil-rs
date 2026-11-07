// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Validation of an alert (its `env:` values and client: `prepare_alerts`).

use vigil_types::plan::AlertConfig;

pub(super) fn validate_alert(alert: &AlertConfig) -> anyhow::Result<()> {
    anyhow::ensure!(!alert.url.is_empty(), "url must be set");
    anyhow::ensure!(
        alert.retry_attempts != Some(0),
        "retry-attempts must be at least 1"
    );
    if let Some(t) = &alert.body_template {
        minijinja::Environment::new()
            .template_from_str(t)
            .map_err(|e| anyhow::anyhow!("body-template: {e}"))?;
    }
    for b in &alert.retry_backoff {
        crate::duration::parse_field("retry-backoff entry", Some(b))?;
    }
    for v in &alert.on_vigild {
        anyhow::ensure!(
            VIGILD_ALERT_EVENTS.contains(&v.as_str()) || v == "*",
            "unknown on-vigild entry {v:?} (expected {})",
            VIGILD_ALERT_EVENTS.join(", ")
        );
    }
    // `env:VAR` values and the HTTP client: `prepare_alerts` in `load`.
    Ok(())
}

/// Names `on-vigild` accepts (besides `"*"`).
const VIGILD_ALERT_EVENTS: &[&str] = &["layers"];
