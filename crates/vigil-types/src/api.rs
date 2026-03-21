// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

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
pub struct DaemonActionRequest {
    pub action: DaemonAction,
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ServiceStatus {
    Active,
    Inactive,
    Backoff,
    Error,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct ServiceInfo {
    pub name: String,
    pub startup: crate::plan::Startup,
    pub current: ServiceStatus,
    pub current_since: Option<DateTime<Utc>>,
    /// Effective stop signal (default: SIGTERM).
    pub stop_signal: String,
    /// Effective on-success policy (default: restart).
    pub on_success: String,
    /// Effective on-failure policy (default: restart).
    pub on_failure: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ServicesAction {
    pub action: ServiceAction,
    pub services: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
    Autostart,
    Replan,
}

// ---------------------------------------------------------------------------
// Changes / Tasks (simplified — no full StateEngine)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChangeStatus {
    Doing,
    Done,
    Error,
    Hold,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct ChangeInfo {
    /// Monotonically increasing change ID.
    pub id: String,
    pub kind: String,
    pub summary: String,
    pub status: ChangeStatus,
    pub spawn_time: DateTime<Utc>,
    pub ready_time: Option<DateTime<Utc>>,
    /// Error message if status is `error`.
    pub err: Option<String>,
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Up,
    Down,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct CheckInfo {
    pub name: String,
    pub level: crate::plan::CheckLevel,
    pub status: CheckStatus,
    /// Consecutive failures since last success.
    pub failures: u32,
    /// Failures required to declare the check down.
    pub threshold: u32,
    /// Seconds until the next scheduled check run. `null` during initial delay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_in_secs: Option<u64>,
}

// ---------------------------------------------------------------------------
// Alerts
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct AlertCheckStatus {
    pub check: String,
    /// Last known status (`null` = no event observed yet).
    pub status: Option<CheckStatus>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct AlertInfo {
    pub name: String,
    /// Raw URL from config (may contain `"env:VAR"` placeholder).
    pub url: String,
    pub format: crate::plan::AlertFormat,
    /// Check names that trigger this alert.
    pub on_check: Vec<String>,
    /// Last known status per watched check.
    pub check_status: Vec<AlertCheckStatus>,
}

// ---------------------------------------------------------------------------
// Exec
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ExecRequest {
    pub command: Vec<String>,
    #[serde(default)]
    pub environment: std::collections::HashMap<String, String>,
    pub working_dir: Option<String>,
    pub user: Option<String>,
    pub group: Option<String>,
    pub timeout: Option<String>,
    pub service_context: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub service: String,
    pub stream: LogStream,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Notices
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoticeType {
    ChangeUpdate,
    CheckFailed,
    CheckRecovered,
    Custom,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Notice {
    pub id: String,
    pub r#type: NoticeType,
    pub key: String,
    pub first_occurred: DateTime<Utc>,
    pub last_occurred: DateTime<Utc>,
    pub occurrences: u64,
}
