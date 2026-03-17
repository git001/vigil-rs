use crate::signal::StopSignal;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Startup {
    #[default]
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnExit {
    Restart,
    Shutdown,
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
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// Custom stop signal (default: SIGTERM).
    /// This is the key feature from haproxytech/pebble PR#720.
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
}

// ---------------------------------------------------------------------------
// Check
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u32>,
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
// LogTarget
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LogTargetConfig {
    #[serde(default)]
    pub override_mode: Override,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub labels: IndexMap<String, String>,
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
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub services: IndexMap<String, ServiceConfig>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub checks: IndexMap<String, CheckConfig>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub log_targets: IndexMap<String, LogTargetConfig>,
}

/// The merged, resolved plan built from all layers in order.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub layers: Vec<Layer>,
    pub services: IndexMap<String, ServiceConfig>,
    pub checks: IndexMap<String, CheckConfig>,
    pub log_targets: IndexMap<String, LogTargetConfig>,
}
