// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Background delivery worker and HTTP send logic.

mod client;
mod send;
#[cfg(test)]
mod tests;
mod worker;

use std::time::Duration;

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// Built-in default for `alerts.max-queue-depth` (overridable per layer file).
pub const DEFAULT_DELIVERY_QUEUE: usize = 256;

/// Built-in default for `alerts.max-queue-time` (overridable per layer file).
pub const DEFAULT_DELIVERY_AGE: Duration = Duration::from_secs(60);

/// Delivery queue limits of a plan (`alerts.max-queue-depth`,
/// `alerts.max-queue-time`), defaults where unset. The plan check rejects
/// invalid values ([`validate_queue_limits`]).
pub fn queue_limits(plan: &vigil_types::plan::Plan) -> (usize, Duration) {
    let depth = plan.alert_queue_depth.unwrap_or(DEFAULT_DELIVERY_QUEUE);
    let age = plan
        .alert_max_queue_time
        .as_deref()
        .and_then(|s| crate::duration::parse_duration(s).ok())
        .unwrap_or(DEFAULT_DELIVERY_AGE);
    (depth, age)
}

/// `max-queue-depth` must be at least 1 (a channel of 0 cannot exist) and
/// `max-queue-time` a duration of at least 1s (jobs are aged in whole
/// seconds; less would discard every alert).
pub fn validate_queue_limits(plan: &vigil_types::plan::Plan) -> anyhow::Result<()> {
    if let Some(d) = plan.alert_queue_depth {
        anyhow::ensure!(d >= 1, "alerts: max-queue-depth must be at least 1");
    }
    if let Some(t) = &plan.alert_max_queue_time {
        let d =
            crate::duration::parse_field("alerts: max-queue-time", Some(t))?.unwrap_or_default();
        anyhow::ensure!(
            d >= Duration::from_secs(1),
            "alerts: max-queue-time {t:?} must be at least 1s"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Re-exports for alert/mod.rs
// ---------------------------------------------------------------------------

pub(crate) use client::prepare;
pub(super) use worker::{DeliveryJob, delivery_worker};
