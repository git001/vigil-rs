// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::Override;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AlertFormat {
    /// Generic JSON webhook — works with any HTTP(S) endpoint.
    #[default]
    Webhook,
    /// Prometheus Alertmanager v2 API (`POST /api/v2/alerts`).
    Alertmanager,
    /// CNCF CloudEvents 1.0 structured JSON.
    CloudEvents,
    /// OpenTelemetry Protocol HTTP/JSON log record (`POST /v1/logs`).
    OtlpLogs,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct AlertConfig {
    #[serde(default)]
    pub override_mode: Override,
    /// HTTP(S) endpoint to POST alerts to.
    pub url: String,
    #[serde(default)]
    pub format: AlertFormat,
    /// Extra HTTP headers (e.g. `Authorization`).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, String>,
    /// Labels attached to alert payloads.
    /// Values prefixed with `"env:"` are resolved from the process environment
    /// at send time. Example: `cluster: "env:CLUSTER_NAME"`.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub labels: IndexMap<String, String>,
    /// Arbitrary key/value fields included in the alert body
    /// (placement is format-dependent).
    /// Values prefixed with `"env:"` are resolved from the process environment.
    /// Example: `k8s_service: "env:KUBERNETES_SERVICE_NAME"`.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub send_info_fields: IndexMap<String, String>,
    /// Names of checks whose state changes trigger this alert.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_check: Vec<String>,
    /// Skip TLS certificate verification (useful for self-signed certs).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tls_insecure: bool,
    /// PEM file with a CA certificate (or chain) to verify the server's TLS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_ca: Option<std::path::PathBuf>,

    /// Explicit HTTP/HTTPS proxy URL for alert requests.
    /// If absent, `HTTPS_PROXY`, `ALL_PROXY`, and `HTTP_PROXY` env vars are
    /// consulted in that order (same behaviour as the vigil CLI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// PEM CA certificate (or chain) to verify the proxy's TLS connection.
    /// Useful for corporate MITM proxies with a custom root CA.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_ca: Option<std::path::PathBuf>,
    /// Comma-separated list of hosts that bypass the proxy.
    /// Example: `"internal.corp, .dev.local"`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_proxy: Option<String>,

    /// Maximum number of send attempts (1 = no retry). Default: 3.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_attempts: Option<u32>,
    /// Delays between retry attempts as duration strings (e.g. `["1s", "2s"]`).
    /// The list length should be `retry_attempts - 1`. If shorter, the last
    /// entry is reused. Default: `["1s", "2s"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retry_backoff: Vec<String>,

    /// Jinja2-style body template for the `webhook` format.
    ///
    /// When set, the template is rendered and sent as the HTTP body instead of
    /// the default `{"check": …, "status": …, …}` payload.  The rendered
    /// string **must** be valid JSON.  On render or parse errors vigild logs a
    /// warning and falls back to the default webhook payload.
    ///
    /// Available template variables:
    /// * `check`     — name of the check (string)
    /// * `status`    — `"up"` or `"down"` (string)
    /// * `timestamp` — RFC 3339 timestamp (string)
    /// * `labels`    — resolved `labels` map (object)
    /// * `info`      — resolved `send_info_fields` map (object)
    ///
    /// Example (Slack incoming webhook):
    /// ```yaml
    /// body_template: '{"text": "Check *{{ check }}* is {{ status }}"}'
    /// ```
    ///
    /// Ignored for formats other than `webhook`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_template: Option<String>,
}

/// The `alerts:` block in a layer file.
///
/// Global queue settings (`max-queue-depth`, `max-queue-time`) are optional
/// top-level keys in this block and apply to all alerts. Individual alert
/// configurations are keyed by their name.
///
/// ```yaml
/// alerts:
///   max-queue-depth: 256   # optional — max pending delivery jobs
///   max-queue-time:  60s   # optional — discard stale jobs after this age
///
///   my-webhook:
///     url: https://hooks.example.com/alert
///     format: webhook
///     on-check: [website-alive]
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct AlertsBlock {
    /// Maximum number of delivery jobs waiting in the queue. New jobs are
    /// dropped (with a warning) when the queue is full. Default: 256.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_queue_depth: Option<usize>,
    /// Maximum time a delivery job may wait before being discarded (e.g. "60s",
    /// "2m"). Prevents a flood of stale alerts after an endpoint comes back
    /// online. Default: "60s".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_queue_time: Option<String>,
    /// Named alert configurations.
    #[serde(flatten)]
    pub entries: IndexMap<String, AlertConfig>,
}

pub(super) fn merge_alert(base: &mut AlertConfig, overlay: &AlertConfig) {
    macro_rules! opt {
        ($f:ident) => {
            if overlay.$f.is_some() {
                base.$f = overlay.$f.clone();
            }
        };
    }

    if !overlay.url.is_empty() {
        base.url = overlay.url.clone();
    }
    base.format = overlay.format;
    base.tls_insecure = overlay.tls_insecure;
    opt!(tls_ca);
    opt!(proxy);
    opt!(proxy_ca);
    opt!(no_proxy);
    opt!(retry_attempts);
    opt!(body_template);
    if !overlay.retry_backoff.is_empty() {
        base.retry_backoff = overlay.retry_backoff.clone();
    }
    base.headers
        .extend(overlay.headers.iter().map(|(k, v)| (k.clone(), v.clone())));
    base.labels
        .extend(overlay.labels.iter().map(|(k, v)| (k.clone(), v.clone())));
    base.send_info_fields.extend(
        overlay
            .send_info_fields
            .iter()
            .map(|(k, v)| (k.clone(), v.clone())),
    );
    for c in &overlay.on_check {
        if !base.on_check.contains(c) {
            base.on_check.push(c.clone());
        }
    }
}
