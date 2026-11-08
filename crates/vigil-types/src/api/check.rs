// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Checks, health and alerts.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Up,
    Down,
    /// `startup: disabled`: the check does not run (only `GET /v1/checks`
    /// reports it).
    Inactive,
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
    /// Why the last run failed, e.g. `HTTP 503 Service Unavailable` or
    /// `exit status 1: <output>`. Absent once a run passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Result of `GET /v1/health`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct HealthInfo {
    /// No selected, running check is down.
    pub healthy: bool,
}

/// Body of `POST /v1/checks/refresh`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CheckRefreshRequest {
    /// The check to run now.
    pub name: String,
}

/// Result of `POST /v1/checks/refresh`: the check after the run, and why
/// the run failed.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct CheckRefresh {
    pub info: CheckInfo,
    /// Why this run failed; absent when it passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    /// Service names whose failures trigger this alert (`*` = all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_service: Vec<String>,
    /// vigild events that trigger this alert (`layers`, `*`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_vigild: Vec<String>,
    /// Last known status per watched check.
    pub check_status: Vec<AlertCheckStatus>,
}
