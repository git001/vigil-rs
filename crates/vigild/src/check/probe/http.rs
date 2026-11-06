// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::time::Duration;

use reqwest::Client as HttpClient;
use tokio::time::timeout;
use tracing::{debug, warn};
use vigil_types::plan::HttpCheck;

use vigil_tls::{ClientTls, PemFile};

pub(super) async fn probe_http(
    client: &HttpClient,
    check: &HttpCheck,
    timeout_dur: Duration,
) -> bool {
    // Build a per-check client only when TLS options require it; reuse the
    // shared client for the common case (no insecure / no custom CA).
    let owned;
    let effective_client: &HttpClient = if check.insecure
        || check.insecure_proxy
        || check.ca.is_some()
        || check.no_follow_redirects
    {
        // Note: reqwest has no per-proxy TLS config; danger_accept_invalid_certs
        // disables verification for all TLS connections in this client (proxy + target).
        let b = HttpClient::builder()
            .timeout(timeout_dur)
            .redirect(if check.no_follow_redirects {
                reqwest::redirect::Policy::none()
            } else {
                reqwest::redirect::Policy::default()
            });
        let tls = ClientTls {
            insecure: check.insecure || check.insecure_proxy,
            ca: check.ca.iter().map(|p| PemFile::new("ca", p)).collect(),
            ..Default::default()
        };
        let b = match tls.apply(b) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %format!("{e:#}"), "http check: TLS settings");
                return false;
            }
        };
        owned = b.build().unwrap_or_default();
        &owned
    } else {
        client
    };

    let mut req = effective_client.get(&check.url);
    for (name, value) in &check.headers {
        req = req.header(name.as_str(), value.as_str());
    }
    match timeout(timeout_dur, req.send()).await {
        Ok(Ok(resp)) => {
            let code = resp.status().as_u16();
            let passed = if check.success_statuses.is_empty() {
                resp.status().is_success()
            } else {
                check.success_statuses.contains(&code)
            };
            debug!(url = %check.url, status = code, passed, "http probe");
            passed
        }
        Ok(Err(e)) => {
            debug!(url = %check.url, error = %e, "http probe failed");
            false
        }
        Err(_) => {
            debug!(url = %check.url, "http probe timed out");
            false
        }
    }
}
