// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Check actors: periodic probes that report service health to the Overlord.

mod actor;
mod probe;

use std::sync::Arc;

use indexmap::IndexMap;
use tokio::sync::{mpsc, oneshot};
use vigil_types::api::{CheckInfo, CheckStatus};
use vigil_types::plan::{CheckConfig, ServiceConfig};

use crate::metrics::MetricsStore;

/// Shortest allowed check `period`: every period runs a probe (an exec
/// check forks a process), so a tiny period would flood the pod or the
/// target.
pub const MIN_PERIOD: std::time::Duration = std::time::Duration::from_secs(1);
/// Longest allowed check `period`.
pub const MAX_PERIOD: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Check a check's `period` (at load and replan).
pub fn validate_period(cfg: &CheckConfig) -> anyhow::Result<()> {
    if let Some(p) = &cfg.period {
        let d = crate::duration::parse_duration(p)
            .map_err(|e| anyhow::anyhow!("invalid period {p:?}: {e}"))?;
        anyhow::ensure!(
            (MIN_PERIOD..=MAX_PERIOD).contains(&d),
            "period {p:?} must be between 1s and 24h"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

pub enum Cmd {
    GetStatus(oneshot::Sender<CheckInfo>),
    Shutdown,
}

pub struct CheckEvent {
    pub check: String,
    pub status: CheckStatus,
    /// Set for every successful probe: when that probe started. Used for
    /// `ready: check`; status-only events (first run, Down) carry `None`.
    pub passed_at: Option<std::time::Instant>,
}

pub struct Handle {
    pub tx: mpsc::Sender<Cmd>,
}

pub fn spawn(
    name: String,
    config: CheckConfig,
    service_configs: Arc<IndexMap<String, ServiceConfig>>,
    event_tx: mpsc::Sender<CheckEvent>,
    metrics: Arc<MetricsStore>,
    initial_status: CheckStatus,
) -> Handle {
    let (tx, rx) = mpsc::channel(16);
    tokio::spawn(actor::run(
        name,
        config,
        service_configs,
        rx,
        event_tx,
        metrics,
        initial_status,
    ));
    Handle { tx }
}
