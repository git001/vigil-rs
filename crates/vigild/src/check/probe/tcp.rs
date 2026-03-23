// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::time::Duration;

use tokio::time::timeout;
use tracing::debug;

pub(super) async fn probe_tcp(host: &str, port: u16, timeout_dur: Duration) -> bool {
    let addr = format!("{}:{}", host, port);
    let passed = matches!(
        timeout(timeout_dur, tokio::net::TcpStream::connect(&addr)).await,
        Ok(Ok(_))
    );
    debug!(addr = %addr, passed, "tcp probe");
    passed
}
