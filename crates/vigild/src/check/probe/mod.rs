// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Check probe implementations: HTTP, TCP, exec.

mod connect_timer;
mod exec;
pub(super) mod http;
mod tcp;

pub use connect_timer::CheckClient;

#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use indexmap::IndexMap;
use vigil_types::plan::{CheckConfig, ServiceConfig};

/// The outcome of one probe run: `Err` says why it failed, e.g.
/// `HTTP 503 Service Unavailable` or `exit status 2: <output>`.
pub(crate) type ProbeResult = Result<(), String>;

/// One probe run: its result and how long the connection took to come up
/// (http, tcp; `None` for exec and when no connection came up).
pub(crate) struct Probe {
    pub result: ProbeResult,
    pub connect: Option<Duration>,
}

pub(super) async fn perform(
    config: &CheckConfig,
    timeout_dur: Duration,
    http: Option<&CheckClient>,
    service_configs: &IndexMap<String, ServiceConfig>,
) -> Probe {
    if let Some(h) = &config.http {
        let Some(client) = http else {
            // `prepare_checks` builds one for every HTTP check.
            tracing::error!(url = %h.url, "http check without a prepared client");
            return Probe {
                result: Err("internal error: no HTTP client for this check".into()),
                connect: None,
            };
        };
        client.reset_connect();
        let result = http::probe_http(client, h, timeout_dur).await;
        return Probe {
            result,
            connect: client.connect_time(),
        };
    }
    if let Some(t) = &config.tcp {
        let host = t.host.as_deref().unwrap_or("localhost");
        let started = Instant::now();
        let result = tcp::probe_tcp(host, t.port, timeout_dur).await;
        // The probe is the connect.
        let connect = result.is_ok().then(|| started.elapsed());
        return Probe { result, connect };
    }
    let result = match &config.exec {
        Some(e) => exec::probe_exec(e, timeout_dur, service_configs).await,
        None => Ok(()),
    };
    Probe {
        result,
        connect: None,
    }
}

/// `timed out after 3s`: the reason of a probe that ran out of time.
fn timed_out(timeout_dur: Duration) -> String {
    format!("timed out after {timeout_dur:?}")
}
