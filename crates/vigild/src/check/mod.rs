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
