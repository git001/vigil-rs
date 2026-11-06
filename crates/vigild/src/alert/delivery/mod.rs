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

// ---------------------------------------------------------------------------
// Re-exports for alert/mod.rs
// ---------------------------------------------------------------------------

pub(crate) use client::prepare;
pub(super) use worker::{DeliveryJob, delivery_worker};
