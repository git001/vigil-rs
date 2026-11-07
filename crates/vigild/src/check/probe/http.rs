// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::time::Duration;

use tokio::time::timeout;
use tracing::debug;
use vigil_types::plan::HttpCheck;

use super::{CheckClient, ProbeResult, timed_out};

/// The client of an HTTP check: its redirect policy, `tls:` and `proxy:`.
/// Reads the `tls:` files: built once when the layers are loaded, off the
/// async runtime.
pub(crate) fn client(check: &HttpCheck) -> anyhow::Result<CheckClient> {
    let builder = reqwest::Client::builder().redirect(if check.no_follow_redirects {
        reqwest::redirect::Policy::none()
    } else {
        reqwest::redirect::Policy::default()
    });
    let builder =
        crate::tls::configure_http_client(builder, check.tls.as_ref(), check.proxy.as_ref())?;
    Ok(CheckClient::build(builder)?)
}

/// The innermost source of `e`: `Connection refused (os error 111)`,
/// `invalid peer certificate: UnknownIssuer`, `failed to lookup address
/// information: …`. reqwest's and hyper's wrappers around it ("error
/// sending request for url (…)", "client error (Connect)") add nothing the
/// check's URL does not say.
fn root_cause(e: &(dyn std::error::Error + 'static)) -> String {
    let mut cause = e;
    while let Some(source) = cause.source() {
        cause = source;
    }
    cause.to_string()
}

pub(super) async fn probe_http(
    client: &CheckClient,
    check: &HttpCheck,
    timeout_dur: Duration,
) -> ProbeResult {
    let mut req = client.http.get(&check.url);
    for (name, value) in &check.headers {
        req = req.header(name.as_str(), value.as_str());
    }
    let result = match timeout(timeout_dur, req.send()).await {
        Ok(Ok(resp)) => {
            let status = resp.status();
            let passed = if check.success_statuses.is_empty() {
                status.is_success()
            } else {
                check.success_statuses.contains(&status.as_u16())
            };
            if passed {
                Ok(())
            } else {
                Err(format!("HTTP {status}"))
            }
        }
        Ok(Err(e)) => Err(format!("GET {}: {}", check.url, root_cause(&e))),
        Err(_) => Err(format!("GET {}: {}", check.url, timed_out(timeout_dur))),
    };
    debug!(url = %check.url, error = result.as_ref().err(), "http probe");
    result
}
