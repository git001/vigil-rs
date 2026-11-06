// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::time::Duration;

use reqwest::Client as HttpClient;
use tokio::time::timeout;
use tracing::debug;
use vigil_types::plan::HttpCheck;

/// The client of an HTTP check: its redirect policy, `tls:` and `proxy:`.
/// Reads the `tls:` files: built once when the layers are loaded, off the
/// async runtime.
pub(crate) fn client(check: &HttpCheck) -> anyhow::Result<HttpClient> {
    let builder = HttpClient::builder().redirect(if check.no_follow_redirects {
        reqwest::redirect::Policy::none()
    } else {
        reqwest::redirect::Policy::default()
    });
    crate::tls::build_http_client(builder, check.tls.as_ref(), check.proxy.as_ref())
}

pub(super) async fn probe_http(
    client: &HttpClient,
    check: &HttpCheck,
    timeout_dur: Duration,
) -> bool {
    let mut req = client.get(&check.url);
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
