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

/// Check a check's `period` (at load and replan): 1s to 24h, see
/// [`crate::duration::parse_interval`].
pub fn validate_period(cfg: &CheckConfig) -> anyhow::Result<()> {
    if let Some(p) = &cfg.period {
        crate::duration::parse_interval(p).map_err(|e| anyhow::anyhow!("period {e}"))?;
    }
    Ok(())
}

/// The HTTP clients of the HTTP checks, by check name.
pub type PreparedChecks = IndexMap<String, reqwest::Client>;

/// Build the client of every HTTP check (its `tls:` and `proxy:`), once
/// when the layers are loaded: an unreadable `tls.ca` or a bad proxy URL is
/// reported there, never at probe time. Reads files: call it off the async
/// runtime.
pub fn prepare_checks(checks: &IndexMap<String, CheckConfig>) -> anyhow::Result<PreparedChecks> {
    checks
        .iter()
        .filter_map(|(name, c)| c.http.as_ref().map(|h| (name, h)))
        .map(|(name, h)| {
            probe::http::client(h)
                .map(|client| (name.clone(), client))
                .map_err(|e| anyhow::anyhow!("check {name:?}: {e:#}"))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

pub enum Cmd {
    GetStatus(oneshot::Sender<CheckInfo>),
    /// The HTTP client built by a replan (it re-reads `tls.ca` and the
    /// other files): replaces the actor's client and keeps its status
    /// and failure count.
    SetHttpClient(reqwest::Client),
    Shutdown,
}

pub struct CheckEvent {
    pub check: String,
    pub status: CheckStatus,
    /// Set for every successful probe: when that probe started. Used for
    /// `ready: check`; status-only events (first run, Down) carry `None`.
    pub passed_at: Option<std::time::Instant>,
    /// Why the check is down: set on the event that reports `Down`.
    pub error: Option<String>,
}

pub struct Handle {
    pub tx: mpsc::Sender<Cmd>,
}

pub fn spawn(
    name: String,
    config: CheckConfig,
    http: Option<reqwest::Client>,
    service_configs: Arc<IndexMap<String, ServiceConfig>>,
    event_tx: mpsc::Sender<CheckEvent>,
    metrics: Arc<MetricsStore>,
    initial_status: CheckStatus,
) -> Handle {
    let (tx, rx) = mpsc::channel(16);
    tokio::spawn(actor::run(
        name,
        actor::Setup { config, http },
        service_configs,
        rx,
        event_tx,
        metrics,
        initial_status,
    ));
    Handle { tx }
}
