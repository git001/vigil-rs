// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors
//
// Integration tests for outgoing HTTP — alert webhook delivery and
// HTTP health-check requests — using wiremock as the target server.

use std::time::Duration;

use indexmap::IndexMap;
use vigil_types::api::CheckStatus;
use vigil_types::plan::{AlertConfig, AlertFormat};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use vigild::alert::AlertSender;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal AlertConfig pointing at `url/hook`.
fn alert_cfg(url: &str) -> AlertConfig {
    AlertConfig {
        url: format!("{url}/hook"),
        format: AlertFormat::Webhook,
        on_check: vec!["my-check".to_string()],
        ..Default::default()
    }
}

fn sender_with_url(url: &str) -> AlertSender {
    let mut sender = AlertSender::new();
    let mut alerts = IndexMap::new();
    alerts.insert("my-alert".to_string(), alert_cfg(url));
    sender.update_alerts(alerts);
    sender.spawn_worker();
    sender
}

/// Wait up to `timeout` for `mock_server` to receive at least `min_count`
/// requests, polling every 10 ms.
async fn wait_for_requests(mock_server: &MockServer, min_count: usize, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let n = mock_server
            .received_requests()
            .await
            .map(|v| v.len())
            .unwrap_or(0);
        if n >= min_count {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

mod alerts;
mod alertmanager;
