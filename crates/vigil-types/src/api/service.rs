// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Services, service actions, changes and signals.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ServiceStatus {
    /// Process is running but not ready yet (see `ready` in the service
    /// config). Dependent services wait until it is `active`.
    Starting,
    Active,
    Inactive,
    Backoff,
    Error,
    /// A `oneshot` service finished successfully.
    Done,
}

/// How a service process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct ExitInfo {
    /// Exit code, if the process exited normally.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    /// Signal number, if the process was killed by a signal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
}

impl ExitInfo {
    /// Shell-style code: the exit code, or `128 + signal` for a signal.
    /// `None` if neither is known (the wait itself failed).
    pub fn shell_code(&self) -> Option<i32> {
        self.code.or(self.signal.map(|s| 128 + s))
    }
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
    /// How the last process of this service ended (raw, before
    /// `exit-code-map`). Absent until the service has exited once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_exit: Option<ExitInfo>,
    /// Last `STATUS=` text the service sent via sd_notify (`ready: notify`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
    /// Next scheduled run (`schedule`); absent for unscheduled services.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run: Option<DateTime<Utc>>,
    /// Dependencies (`after`, `before`, `requires`) a service queued by
    /// autostart still waits for before it starts; absent otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waiting_for: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
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
// Signals
// ---------------------------------------------------------------------------

/// Body of `POST /v1/signals`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SignalsRequest {
    /// Signal name as in `stop-signal`, e.g. `SIGHUP`, `SIGUSR1`.
    pub signal: String,
    /// The services to send it to; each must be running.
    pub services: Vec<String>,
}

/// Result of `POST /v1/signals`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct SignalsResult {
    pub signal: String,
    /// The services that got the signal.
    pub services: Vec<String>,
}
