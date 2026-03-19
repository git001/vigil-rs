// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::signal::StopSignal;

// ---------------------------------------------------------------------------
// Override semantics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Override {
    #[default]
    Merge,
    Replace,
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Startup {
    #[default]
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogsForward {
    /// Forward service stdout/stderr to vigild's stdout/stderr (default).
    /// Allows `podman logs` / `docker logs` to capture service output.
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

impl Default for LogsForward {
    fn default() -> Self {
        LogsForward::Enabled
    }
}

/// Format used when pushing log entries to an external socket/address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogsPushFormat {
    /// Newline-delimited JSON — one `LogEntry` JSON object per line.
    Ndjson,
}

impl Default for LogsPushFormat {
    fn default() -> Self {
        LogsPushFormat::Ndjson
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnExit {
    Restart,
    /// Shut down the daemon; exit code depends on context (success→0, failure→10).
    Shutdown,
    /// Shut down the daemon with exit code 10 (intended for on-success).
    FailureShutdown,
    /// Shut down the daemon with exit code 0 (intended for on-failure).
    SuccessShutdown,
    Ignore,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ServiceConfig {
    #[serde(default)]
    pub override_mode: Override,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub startup: Startup,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub environment: IndexMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// Custom stop signal (default: SIGTERM).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_signal: Option<StopSignal>,
    /// Grace period before SIGKILL after stop_signal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kill_delay: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_success: Option<OnExit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<OnExit>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub on_check_failure: IndexMap<String, OnExit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backoff_delay: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backoff_factor: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backoff_limit: Option<String>,
    /// Controls how vigild handles stdout/stderr. Default: `enabled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logs_forward: Option<LogsForward>,
    /// Unix socket path to connect to and push log entries (ndjson by default).
    /// Mutually exclusive with `logs_push_addr`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_push_socket: Option<String>,
    /// TCP address (`host:port`) to connect to and push log entries (ndjson by default).
    /// Mutually exclusive with `logs_push_socket`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_push_addr: Option<String>,
    /// Format for pushed log lines. Default: `ndjson`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_push_format: Option<LogsPushFormat>,
}

// ---------------------------------------------------------------------------
// Check
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CheckLevel {
    #[default]
    Alive,
    Ready,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct HttpCheck {
    pub url: String,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TcpCheck {
    pub host: Option<String>,
    pub port: u16,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ExecCheck {
    pub command: String,
    /// Inherit env/user/group/working-dir from this service; check settings override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_context: Option<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub environment: IndexMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CheckConfig {
    #[serde(default)]
    pub override_mode: Override,
    #[serde(default)]
    pub level: CheckLevel,
    #[serde(default)]
    pub startup: Startup,
    /// Initial delay before the first check is performed (vigil extension).
    /// Useful to avoid false failures while a service is still starting up.
    /// Example: "5s", "500ms". Default: no delay (first check runs immediately).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delay: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tcp: Option<TcpCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exec: Option<ExecCheck>,
}

// ---------------------------------------------------------------------------
// Layer + Plan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Layer {
    #[serde(skip)]
    pub order: u32,
    #[serde(skip)]
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub services: IndexMap<String, ServiceConfig>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub checks: IndexMap<String, CheckConfig>,
}

/// The merged, resolved plan built from all layers in order.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub layers: Vec<Layer>,
    pub services: IndexMap<String, ServiceConfig>,
    pub checks: IndexMap<String, CheckConfig>,
}

impl Plan {
    /// Merge a list of layers (in order) into a resolved Plan.
    pub fn from_layers(layers: Vec<Layer>) -> Self {
        let mut services: IndexMap<String, ServiceConfig> = IndexMap::new();
        let mut checks: IndexMap<String, CheckConfig> = IndexMap::new();

        for layer in &layers {
            for (name, svc) in &layer.services {
                match svc.override_mode {
                    Override::Replace => {
                        services.insert(name.clone(), svc.clone());
                    }
                    Override::Merge => match services.get_mut(name) {
                        None => {
                            services.insert(name.clone(), svc.clone());
                        }
                        Some(existing) => merge_service(existing, svc),
                    },
                }
            }

            for (name, chk) in &layer.checks {
                match chk.override_mode {
                    Override::Replace => {
                        checks.insert(name.clone(), chk.clone());
                    }
                    Override::Merge => match checks.get_mut(name) {
                        None => {
                            checks.insert(name.clone(), chk.clone());
                        }
                        Some(existing) => merge_check(existing, chk),
                    },
                }
            }
        }

        Plan { layers, services, checks }
    }
}

/// Merge `overlay` into `base` (Override::Merge semantics).
fn merge_service(base: &mut ServiceConfig, overlay: &ServiceConfig) {
    macro_rules! opt {
        ($f:ident) => {
            if overlay.$f.is_some() {
                base.$f = overlay.$f.clone();
            }
        };
    }

    opt!(summary);
    opt!(description);
    opt!(command);
    base.startup = overlay.startup;
    for v in &overlay.after {
        if !base.after.contains(v) {
            base.after.push(v.clone());
        }
    }
    for v in &overlay.before {
        if !base.before.contains(v) {
            base.before.push(v.clone());
        }
    }
    for v in &overlay.requires {
        if !base.requires.contains(v) {
            base.requires.push(v.clone());
        }
    }
    base.environment.extend(overlay.environment.iter().map(|(k, v)| (k.clone(), v.clone())));
    opt!(user);
    opt!(user_id);
    opt!(group);
    opt!(group_id);
    opt!(working_dir);
    opt!(stop_signal);
    opt!(kill_delay);
    opt!(on_success);
    opt!(on_failure);
    base.on_check_failure
        .extend(overlay.on_check_failure.iter().map(|(k, v)| (k.clone(), *v)));
    opt!(backoff_delay);
    opt!(backoff_factor);
    opt!(backoff_limit);
    opt!(logs_forward);
    opt!(logs_push_socket);
    opt!(logs_push_addr);
    opt!(logs_push_format);
}

fn merge_check(base: &mut CheckConfig, overlay: &CheckConfig) {
    base.level = overlay.level;
    base.startup = overlay.startup;

    macro_rules! opt {
        ($f:ident) => {
            if overlay.$f.is_some() {
                base.$f = overlay.$f.clone();
            }
        };
    }

    opt!(delay);
    opt!(period);
    opt!(timeout);
    opt!(threshold);
    opt!(http);
    opt!(tcp);
    opt!(exec);
}
