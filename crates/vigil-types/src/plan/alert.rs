// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{Override, append_unique};

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

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AlertConfig {
    /// `override: merge|replace` in the layer file.
    #[serde(default, rename = "override")]
    pub override_mode: Override,
    /// HTTP(S) endpoint to POST alerts to.
    pub url: String,
    /// `None`: not set in this layer (a merge keeps the earlier value);
    /// read it with [`AlertConfig::format`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<AlertFormat>,
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
    /// Names of checks whose state changes trigger this alert. `"*"` means
    /// every check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_check: Vec<String>,
    /// Names of services whose failures (and recovery) trigger this alert.
    /// `"*"` means every service.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_service: Vec<String>,
    /// vigild events that trigger this alert: `layers` (invalid layers
    /// with `--watch-layers`) or `"*"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_vigild: Vec<String>,
    /// Skip TLS certificate verification (useful for self-signed certs).
    /// `None`: not set in this layer; read it with
    /// [`AlertConfig::tls_insecure`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_insecure: Option<bool>,
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
    /// the default `{"source": …, "name": …, "status": …, …}` payload.  The rendered
    /// string **must** be valid JSON. A syntax error makes the layer invalid;
    /// on render errors or invalid JSON vigild logs a warning and falls back
    /// to the default webhook payload.
    ///
    /// Available template variables:
    /// * `source`    — `"check"`, `"service"` or `"vigild"` (string)
    /// * `name`      — name of the check or service, `"layers"` for vigild
    /// * `event`     — e.g. `"failed"`, `"backoff"`, `"recovered"` (string)
    /// * `status`    — `"up"` or `"down"` (string)
    /// * `message`   — detail, e.g. `"exited with code 3, restarting in 2s"`
    /// * `check`     — name of the check; undefined for other sources
    /// * `timestamp` — RFC 3339 timestamp (string)
    /// * `labels`    — resolved `labels` map (object)
    /// * `info`      — resolved `send_info_fields` map (object)
    ///
    /// Example (Slack incoming webhook):
    /// ```yaml
    /// body-template: '{"text": "Check *{{ check }}* is {{ status }}"}'
    /// ```
    ///
    /// Ignored for formats other than `webhook`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_template: Option<String>,
}

impl AlertConfig {
    /// Effective `format` (default `webhook`).
    pub fn format(&self) -> AlertFormat {
        self.format.unwrap_or_default()
    }

    /// Effective `tls-insecure` (default `false`).
    pub fn tls_insecure(&self) -> bool {
        self.tls_insecure.unwrap_or(false)
    }
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
    if !overlay.url.is_empty() {
        base.url = overlay.url.clone();
    }
    merge_opt!(
        base,
        overlay,
        [
            format,
            tls_insecure,
            tls_ca,
            proxy,
            proxy_ca,
            no_proxy,
            retry_attempts,
            body_template
        ]
    );
    if !overlay.retry_backoff.is_empty() {
        base.retry_backoff = overlay.retry_backoff.clone();
    }
    base.headers.extend(overlay.headers.clone());
    base.labels.extend(overlay.labels.clone());
    base.send_info_fields
        .extend(overlay.send_info_fields.clone());
    append_unique(&mut base.on_check, &overlay.on_check);
    append_unique(&mut base.on_service, &overlay.on_service);
    append_unique(&mut base.on_vigild, &overlay.on_vigild);
}
