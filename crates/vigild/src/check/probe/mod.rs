// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Check probe implementations: HTTP, TCP, exec.

mod exec;
pub(super) mod http;
mod tcp;

#[cfg(test)]
mod tests;

use std::time::Duration;

use indexmap::IndexMap;
use reqwest::Client as HttpClient;
use vigil_types::plan::{CheckConfig, ServiceConfig};

pub(super) async fn perform(
    config: &CheckConfig,
    timeout_dur: Duration,
    http: Option<&HttpClient>,
    service_configs: &IndexMap<String, ServiceConfig>,
) -> bool {
    if let Some(h) = &config.http {
        let Some(client) = http else {
            // `prepare_checks` builds one for every HTTP check.
            tracing::error!(url = %h.url, "http check without a prepared client");
            return false;
        };
        return http::probe_http(client, h, timeout_dur).await;
    }
    if let Some(t) = &config.tcp {
        let host = t.host.as_deref().unwrap_or("localhost");
        return tcp::probe_tcp(host, t.port, timeout_dur).await;
    }
    if let Some(e) = &config.exec {
        return exec::probe_exec(e, timeout_dur, service_configs).await;
    }
    true
}
