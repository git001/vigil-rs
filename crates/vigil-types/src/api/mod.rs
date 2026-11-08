// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Request and response types of the vigild HTTP API.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

mod check;
mod service;
#[cfg(test)]
mod tests;

pub use check::{
    AlertCheckStatus, AlertInfo, CheckInfo, CheckRefresh, CheckRefreshRequest, CheckStatus,
    HealthInfo,
};
pub use service::{
    ChangeInfo, ChangeStatus, ExitInfo, ServiceAction, ServiceInfo, ServiceStatus, ServicesAction,
    SignalsRequest, SignalsResult,
};

// ---------------------------------------------------------------------------
// Generic envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct Response<T> {
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(rename = "status-code")]
    pub status_code: u16,
    pub status: String,
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// System info
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct SystemInfo {
    /// UUID generated fresh at each daemon start.
    #[schema(format = Uuid)]
    pub boot_id: String,
    /// Address of the HTTP API (Unix socket path).
    pub http_address: String,
    /// Address of the HTTPS API, if TLS is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub https_address: Option<String>,
    /// vigild version string.
    pub version: String,
    /// Timestamp when the daemon started.
    pub start_time: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Daemon control
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DaemonAction {
    Stop,
    Restart,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DaemonActionRequest {
    pub action: DaemonAction,
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub service: String,
    pub stream: LogStream,
    pub message: String,
}
