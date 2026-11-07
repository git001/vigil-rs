// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The value enums of a service: startup, type, readiness, logs, exit
//! policies.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Startup {
    #[default]
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogsForward {
    /// Forward service stdout/stderr to vigild's stdout/stderr (default).
    /// Allows `podman logs` / `docker logs` to capture service output.
    #[default]
    Enabled,
    /// Keep service logs internal (accessible via `vigil logs` / the API).
    /// The service's stdout/stderr is captured and stored in the log buffer
    /// but NOT printed to vigild's own stdout/stderr.
    Disabled,
    /// Let the service's stdout/stderr pass directly to the container's
    /// stdout/stderr without any capture or buffering by vigild.
    /// Use this for log-collector services (Vector, Filebeat, …) that
    /// format and emit their own output — vigild must not interfere.
    Passthrough,
}

/// Format used when pushing log entries to an external socket/address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogsPushFormat {
    /// Newline-delimited JSON — one `LogEntry` JSON object per line.
    #[default]
    Ndjson,
}

/// How a service's process is meant to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceType {
    /// A long-running process (default).
    #[default]
    Simple,
    /// A job that runs to completion. Exit success ends in state `done`;
    /// services ordered after it start then. Optionally run on a
    /// `schedule`.
    Oneshot,
}

/// When a started service counts as ready (state `active`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadyMode {
    /// Ready as soon as the process is spawned (v3 behaviour).
    #[default]
    Process,
    /// Ready when the service sends `READY=1` to `$NOTIFY_SOCKET`
    /// (sd_notify protocol).
    Notify,
    /// Ready when the check named in `ready-check` passes for the first time
    /// after the process was spawned.
    Check,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnExit {
    Restart,
    /// Shut down the daemon. Exit code: the service's code with
    /// `--exit-code propagate` (default), else success→0, failure→10.
    Shutdown,
    /// Shut down the daemon with a failure code: the service's non-zero code
    /// with `--exit-code propagate`, else (or for code 0) 10.
    FailureShutdown,
    /// Shut down the daemon with exit code 0.
    SuccessShutdown,
    Ignore,
}

impl OnExit {
    /// The name in the layer files (`failure-shutdown`).
    pub fn as_str(self) -> &'static str {
        match self {
            OnExit::Restart => "restart",
            OnExit::Shutdown => "shutdown",
            OnExit::FailureShutdown => "failure-shutdown",
            OnExit::SuccessShutdown => "success-shutdown",
            OnExit::Ignore => "ignore",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OnExit;

    #[test]
    fn as_str_is_the_layer_name() {
        for policy in [
            OnExit::Restart,
            OnExit::Shutdown,
            OnExit::FailureShutdown,
            OnExit::SuccessShutdown,
            OnExit::Ignore,
        ] {
            let yaml = serde_yaml::to_string(&policy).unwrap();
            assert_eq!(yaml.trim_end(), policy.as_str());
        }
    }
}
