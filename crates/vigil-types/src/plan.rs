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
    /// Syntactic sugar for `after` from the reverse direction.
    /// `B before: [A]` is equivalent to `A after: [B]` — B must be running before A starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    /// Hard dependency: implies `after` ordering AND stops this service if any
    /// listed service leaves the running state (Inactive, Backoff, Error).
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
    /// Skip TLS certificate verification (useful for self-signed certs).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub insecure: bool,
    /// PEM file with a CA certificate (or chain) to verify the server's TLS.
    /// Supports chain files with multiple concatenated PEM blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<std::path::PathBuf>,
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(services: impl IntoIterator<Item = (&'static str, ServiceConfig)>) -> Layer {
        Layer {
            order: 0,
            label: "test".into(),
            summary: None,
            description: None,
            services: services.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
            checks: IndexMap::new(),
        }
    }

    fn svc(command: &str) -> ServiceConfig {
        ServiceConfig { command: Some(command.into()), ..Default::default() }
    }

    // --- single layer ---

    #[test]
    fn empty_layers_produces_empty_plan() {
        let plan = Plan::from_layers(vec![]);
        assert!(plan.services.is_empty());
        assert!(plan.checks.is_empty());
    }

    #[test]
    fn single_layer_is_passed_through() {
        let plan = Plan::from_layers(vec![layer([("app", svc("/bin/app"))])]);
        assert_eq!(plan.services["app"].command.as_deref(), Some("/bin/app"));
    }

    // --- merge: command override ---

    #[test]
    fn later_layer_overrides_command() {
        let base = layer([("app", svc("/bin/v1"))]);
        let overlay = layer([("app", svc("/bin/v2"))]);
        let plan = Plan::from_layers(vec![base, overlay]);
        assert_eq!(plan.services["app"].command.as_deref(), Some("/bin/v2"));
    }

    #[test]
    fn overlay_none_field_does_not_clear_base() {
        let mut base_svc = svc("/bin/app");
        base_svc.user = Some("alice".into());
        let overlay_svc = svc("/bin/app-v2"); // user is None
        let plan = Plan::from_layers(vec![
            layer([("app", base_svc)]),
            layer([("app", overlay_svc)]),
        ]);
        // user from base must survive
        assert_eq!(plan.services["app"].user.as_deref(), Some("alice"));
    }

    // --- merge: lists are union-merged, no duplicates ---

    #[test]
    fn after_lists_are_merged_without_duplicates() {
        let mut s1 = svc("/bin/app");
        s1.after = vec!["db".into(), "cache".into()];
        let mut s2 = svc("/bin/app");
        s2.after = vec!["cache".into(), "queue".into()]; // "cache" already in base
        let plan = Plan::from_layers(vec![layer([("app", s1)]), layer([("app", s2)])]);
        let after = &plan.services["app"].after;
        assert_eq!(after.len(), 3, "expected db, cache, queue — got {after:?}");
        assert!(after.contains(&"db".to_owned()));
        assert!(after.contains(&"cache".to_owned()));
        assert!(after.contains(&"queue".to_owned()));
    }

    #[test]
    fn requires_lists_are_merged() {
        let mut s1 = svc("/bin/app");
        s1.requires = vec!["db".into()];
        let mut s2 = svc("/bin/app");
        s2.requires = vec!["auth".into()];
        let plan = Plan::from_layers(vec![layer([("app", s1)]), layer([("app", s2)])]);
        let req = &plan.services["app"].requires;
        assert!(req.contains(&"db".to_owned()));
        assert!(req.contains(&"auth".to_owned()));
    }

    // --- merge: environment map ---

    #[test]
    fn environment_maps_are_merged_overlay_wins() {
        let mut s1 = svc("/bin/app");
        s1.environment.insert("FOO".into(), "base".into());
        s1.environment.insert("BAR".into(), "base".into());
        let mut s2 = svc("/bin/app");
        s2.environment.insert("FOO".into(), "overlay".into()); // override
        s2.environment.insert("BAZ".into(), "overlay".into()); // new key
        let plan = Plan::from_layers(vec![layer([("app", s1)]), layer([("app", s2)])]);
        let env = &plan.services["app"].environment;
        assert_eq!(env["FOO"], "overlay"); // overlay wins
        assert_eq!(env["BAR"], "base");    // base survives
        assert_eq!(env["BAZ"], "overlay"); // new key added
    }

    // --- replace override ---

    #[test]
    fn replace_override_discards_base() {
        let mut s1 = svc("/bin/v1");
        s1.after = vec!["db".into()];
        s1.environment.insert("KEY".into(), "val".into());

        let mut s2 = svc("/bin/v2");
        s2.override_mode = Override::Replace;

        let plan = Plan::from_layers(vec![layer([("app", s1)]), layer([("app", s2)])]);
        let svc = &plan.services["app"];
        assert_eq!(svc.command.as_deref(), Some("/bin/v2"));
        assert!(svc.after.is_empty(), "replace must clear after list");
        assert!(svc.environment.is_empty(), "replace must clear environment");
    }

    // --- multiple services, independent ---

    #[test]
    fn multiple_services_are_independent() {
        let plan = Plan::from_layers(vec![layer([
            ("web", svc("/bin/web")),
            ("db",  svc("/bin/db")),
        ])]);
        assert_eq!(plan.services.len(), 2);
        assert_eq!(plan.services["web"].command.as_deref(), Some("/bin/web"));
        assert_eq!(plan.services["db"].command.as_deref(),  Some("/bin/db"));
    }

    // --- startup field ---

    #[test]
    fn startup_enabled_propagates() {
        let mut s = svc("/bin/app");
        s.startup = Startup::Enabled;
        let plan = Plan::from_layers(vec![layer([("app", s)])]);
        assert_eq!(plan.services["app"].startup, Startup::Enabled);
    }

    #[test]
    fn startup_can_be_overridden_to_disabled() {
        let mut s1 = svc("/bin/app");
        s1.startup = Startup::Enabled;
        let mut s2 = svc("/bin/app");
        s2.startup = Startup::Disabled;
        let plan = Plan::from_layers(vec![layer([("app", s1)]), layer([("app", s2)])]);
        assert_eq!(plan.services["app"].startup, Startup::Disabled);
    }
}
