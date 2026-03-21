// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Background delivery worker and HTTP send logic.

mod client;
mod send;
#[cfg(test)]
mod tests;
mod worker;

use std::time::Duration;

use vigil_types::plan::AlertConfig;

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// Built-in default for `alerts.max-queue-depth` (overridable per layer file).
pub const DEFAULT_DELIVERY_QUEUE: usize = 256;

/// Built-in default for `alerts.max-queue-time` (overridable per layer file).
pub const DEFAULT_DELIVERY_AGE: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Re-exports for alert/mod.rs
// ---------------------------------------------------------------------------

pub(super) use client::build_client;
pub(super) use worker::{DeliveryJob, delivery_worker};

// ---------------------------------------------------------------------------
// warn_unset_env_vars — kept here as it belongs to delivery configuration
// ---------------------------------------------------------------------------

/// Check all `env:VAR` references in `cfg` and warn for each unset variable.
pub(super) fn warn_unset_env_vars(alert_name: &str, cfg: &AlertConfig) {
    let check = |field: &str, val: &str| {
        if let Some(var) = val.strip_prefix("env:")
            && std::env::var(var).map(|v| v.is_empty()).unwrap_or(true)
        {
            tracing::warn!(
                alert = %alert_name,
                field = %field,
                env_var = %var,
                "alert config references unset env var — field will be empty"
            );
        }
    };

    check("url", &cfg.url);
    if let Some(p) = &cfg.proxy {
        check("proxy", p);
    }
    if let Some(p) = &cfg.no_proxy {
        check("no_proxy", p);
    }
    for (k, v) in &cfg.headers {
        check(&format!("headers.{k}"), v);
    }
    for (k, v) in &cfg.labels {
        check(&format!("labels.{k}"), v);
    }
    for (k, v) in &cfg.send_info_fields {
        check(&format!("send_info_fields.{k}"), v);
    }
}
