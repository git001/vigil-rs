// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Check actors: periodic probes that report service health to the Overlord.

mod actor;
mod probe;

use std::sync::Arc;

use indexmap::IndexMap;
use tokio::sync::{mpsc, oneshot};
use vigil_types::api::{CheckInfo, CheckRefresh, CheckStatus};
use vigil_types::plan::{CheckConfig, ServiceConfig};

use crate::metrics::MetricsStore;

/// The HTTP clients of the HTTP checks, by check name.
pub type PreparedChecks = IndexMap<String, probe::CheckClient>;

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
    SetHttpClient(probe::CheckClient),
    /// Run the probe now and count it like a scheduled run
    /// (`POST /v1/checks/refresh`).
    Refresh(oneshot::Sender<CheckRefresh>),
    Shutdown,
}

/// Run a check that has no actor (`startup: disabled`) once, for
/// `POST /v1/checks/refresh`. Nothing is counted or reported: the result
/// shows only this run (`down` with 1 failure, or `up`).
pub async fn run_once_detached(
    name: String,
    config: CheckConfig,
    http: Option<probe::CheckClient>,
    service_configs: Arc<IndexMap<String, ServiceConfig>>,
) -> CheckRefresh {
    let timeout_dur = actor::timeout_of(&config);
    let probe = probe::perform(&config, timeout_dur, http.as_ref(), &service_configs).await;
    let error = probe.result.err();
    CheckRefresh {
        info: CheckInfo {
            name,
            level: config.level(),
            status: if error.is_some() {
                CheckStatus::Down
            } else {
                CheckStatus::Up
            },
            failures: u32::from(error.is_some()),
            threshold: config.threshold.unwrap_or(actor::DEFAULT_THRESHOLD),
            next_run_in_secs: None,
            last_error: error.clone(),
        },
        error,
    }
}

/// What `GET /v1/checks` reports for a check with `startup: disabled`,
/// which has no actor: `inactive`, no failures, no next run.
pub fn inactive_info(name: String, config: &CheckConfig) -> CheckInfo {
    CheckInfo {
        name,
        level: config.level(),
        status: CheckStatus::Inactive,
        failures: 0,
        threshold: config.threshold.unwrap_or(actor::DEFAULT_THRESHOLD),
        next_run_in_secs: None,
        last_error: None,
    }
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
    http: Option<probe::CheckClient>,
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
