// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Exit policies: the effective `on-success` / `on-failure`, and vigild's
//! exit code when a policy shuts it down.

use vigil_types::plan::{OnExit, ServiceConfig};

/// How vigild's own exit code is chosen when a policy shuts it down
/// (`--exit-code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExitCodeMode {
    /// v3 behaviour: 0 for success, 10 for failure.
    Fixed,
    /// The service's (mapped) exit code; `128 + signal` for a signal.
    #[default]
    Propagate,
}

/// vigild's exit code for a shutdown `policy`.
///
/// `success` is the service's classification (`success-exit-codes`), `code`
/// its mapped shell code (`None` if unknown, e.g. for check failures).
/// Returns `None` for policies that do not shut vigild down.
pub fn shutdown_code(
    mode: ExitCodeMode,
    policy: OnExit,
    success: bool,
    code: Option<i32>,
) -> Option<i32> {
    let fixed = if success { 0 } else { 10 };
    match policy {
        OnExit::Restart | OnExit::Ignore => None,
        OnExit::SuccessShutdown => Some(0),
        OnExit::Shutdown => Some(match mode {
            ExitCodeMode::Fixed => fixed,
            ExitCodeMode::Propagate => code.unwrap_or(fixed),
        }),
        // A failure exit must never be 0.
        OnExit::FailureShutdown => Some(match (mode, code) {
            (ExitCodeMode::Propagate, Some(c)) if c != 0 => c,
            _ => 10,
        }),
    }
}

/// Effective `on-success` policy. `None` for a oneshot without one: it ends
/// in state `done`.
pub fn effective_on_success(cfg: &ServiceConfig) -> Option<OnExit> {
    match (cfg.on_success, cfg.service_type.unwrap_or_default()) {
        (Some(p), _) => Some(p),
        (None, vigil_types::plan::ServiceType::Oneshot) => None,
        (None, vigil_types::plan::ServiceType::Simple) => Some(OnExit::Restart),
    }
}

/// Effective `on-failure` policy: `ignore` for scheduled jobs (the next
/// run comes at the next scheduled time), else `restart`.
pub fn effective_on_failure(cfg: &ServiceConfig) -> OnExit {
    cfg.on_failure.unwrap_or(if cfg.schedule.is_some() {
        OnExit::Ignore
    } else {
        OnExit::Restart
    })
}

#[cfg(test)]
mod shutdown_code_tests {
    use super::*;
    use ExitCodeMode::*;
    use OnExit::*;

    #[test]
    fn fixed_mode_keeps_v3_codes() {
        assert_eq!(shutdown_code(Fixed, Shutdown, true, Some(0)), Some(0));
        assert_eq!(shutdown_code(Fixed, Shutdown, false, Some(3)), Some(10));
        assert_eq!(
            shutdown_code(Fixed, FailureShutdown, true, Some(0)),
            Some(10)
        );
        assert_eq!(
            shutdown_code(Fixed, FailureShutdown, false, Some(3)),
            Some(10)
        );
        assert_eq!(
            shutdown_code(Fixed, SuccessShutdown, false, Some(3)),
            Some(0)
        );
    }

    #[test]
    fn propagate_mode_uses_service_code() {
        assert_eq!(shutdown_code(Propagate, Shutdown, false, Some(3)), Some(3));
        assert_eq!(shutdown_code(Propagate, Shutdown, true, Some(0)), Some(0));
        assert_eq!(
            shutdown_code(Propagate, Shutdown, false, Some(143)),
            Some(143)
        );
        assert_eq!(
            shutdown_code(Propagate, FailureShutdown, false, Some(3)),
            Some(3)
        );
        assert_eq!(
            shutdown_code(Propagate, FailureShutdown, true, Some(0)),
            Some(10),
            "failure-shutdown must not exit 0"
        );
        assert_eq!(
            shutdown_code(Propagate, SuccessShutdown, false, Some(3)),
            Some(0)
        );
    }

    #[test]
    fn unknown_code_falls_back_to_fixed() {
        assert_eq!(shutdown_code(Propagate, Shutdown, false, None), Some(10));
        assert_eq!(
            shutdown_code(Propagate, FailureShutdown, false, None),
            Some(10)
        );
    }

    #[test]
    fn non_shutdown_policies_return_none() {
        assert_eq!(shutdown_code(Propagate, Restart, false, Some(1)), None);
        assert_eq!(shutdown_code(Fixed, Ignore, true, Some(0)), None);
    }
}
