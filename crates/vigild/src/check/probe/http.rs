// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::time::Duration;

use reqwest::Client as HttpClient;
use tokio::time::timeout;
use tracing::warn;
use vigil_types::plan::HttpCheck;

use crate::tls::load_pem_chain;

pub(super) async fn probe_http(
    client: &HttpClient,
    check: &HttpCheck,
    timeout_dur: Duration,
) -> bool {
    // Build a per-check client only when TLS options require it; reuse the
    // shared client for the common case (no insecure / no custom CA).
    let owned;
    let effective_client: &HttpClient = if check.insecure || check.ca.is_some() {
        let mut b = HttpClient::builder()
            .timeout(timeout_dur)
            .danger_accept_invalid_certs(check.insecure);
        if let Some(ca_path) = &check.ca {
            match load_pem_chain(ca_path) {
                Ok(certs) => {
                    for cert in certs {
                        b = b.add_root_certificate(cert);
                    }
                }
                Err(e) => {
                    warn!(path = %ca_path.display(), error = %e, "http check: failed to load ca cert");
                    return false;
                }
            }
        }
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
        Ok(Ok(resp)) => resp.status().is_success(),
        _ => false,
    }
}
