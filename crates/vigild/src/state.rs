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

#[cfg(test)]
mod tests {
    use super::*;
    use vigil_types::api::ServiceStatus;

    #[test]
    fn to_api_status_all_variants() {
        assert_eq!(
            ServiceState::Inactive.to_api_status(),
            ServiceStatus::Inactive
        );
        assert_eq!(
            ServiceState::Starting.to_api_status(),
            ServiceStatus::Active
        );
        assert_eq!(ServiceState::Active.to_api_status(), ServiceStatus::Active);
        assert_eq!(
            ServiceState::Stopping.to_api_status(),
            ServiceStatus::Inactive
        );
        assert_eq!(
            ServiceState::Backoff.to_api_status(),
            ServiceStatus::Backoff
        );
        assert_eq!(ServiceState::Error.to_api_status(), ServiceStatus::Error);
    }

    #[test]
    fn is_running_only_starting_and_active() {
        assert!(ServiceState::Starting.is_running());
        assert!(ServiceState::Active.is_running());
        assert!(!ServiceState::Inactive.is_running());
        assert!(!ServiceState::Stopping.is_running());
        assert!(!ServiceState::Backoff.is_running());
        assert!(!ServiceState::Error.is_running());
    }
}
