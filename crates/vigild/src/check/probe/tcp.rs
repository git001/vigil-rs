// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::time::Duration;

use tokio::time::timeout;
use tracing::debug;

use super::{ProbeResult, timed_out};

pub(super) async fn probe_tcp(host: &str, port: u16, timeout_dur: Duration) -> ProbeResult {
    let addr = format!("{}:{}", host, port);
    let result = match timeout(timeout_dur, tokio::net::TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(format!("connect {addr}: {e}")),
        Err(_) => Err(format!("connect {addr}: {}", timed_out(timeout_dur))),
    };
    debug!(addr = %addr, error = result.as_ref().err(), "tcp probe");
    result
}
