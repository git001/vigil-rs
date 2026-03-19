// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use vigil_types::api::ServiceStatus;

/// Internal lifecycle state of a supervised service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// Not running, no pending restart.
    Inactive,
    /// Process has been spawned; waiting for it to become stable.
    /// Transitions to Active once the process is confirmed running; health checks monitor thereafter.
    Starting,
    /// Process is running normally.
    Active,
    /// Stop signal sent; waiting for the process to exit (or kill-delay to expire).
    Stopping,
    /// Waiting before the next restart attempt.
    Backoff,
    /// Permanent failure: backoff limit exceeded.
    Error,
}

impl ServiceState {
    /// Map to the API-visible status.
    pub fn to_api_status(self) -> ServiceStatus {
        match self {
            ServiceState::Inactive | ServiceState::Stopping => ServiceStatus::Inactive,
            ServiceState::Starting | ServiceState::Active => ServiceStatus::Active,
            ServiceState::Backoff => ServiceStatus::Backoff,
            ServiceState::Error => ServiceStatus::Error,
        }
    }

    pub fn is_running(self) -> bool {
        matches!(self, ServiceState::Starting | ServiceState::Active)
    }
}
