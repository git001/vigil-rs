// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::time::Duration;

use reqwest::Client;
use serde_json::Value;
use tracing::error;
use vigil_types::plan::{AlertConfig, AlertFormat};

use crate::alert::format::resolve;

/// Attempt to POST `body` to `cfg.url`, retrying on connection errors or 5xx.
pub(crate) async fn http_send(client: &Client, cfg: &AlertConfig, body: Value) {
    let max_attempts = cfg.retry_attempts.unwrap_or(3).max(1);

    let default_backoff = vec![Duration::from_secs(1), Duration::from_secs(2)];
    let backoff: Vec<Duration> = if cfg.retry_backoff.is_empty() {
        default_backoff
    } else {
        cfg.retry_backoff
            .iter()
            .map(|s| {
                crate::duration::parse_duration(s).unwrap_or_else(|e| {
                    error!(value = %s, error = %e, "alert: invalid retry_backoff entry, using 1s");
                    Duration::from_secs(1)
                })
            })
            .collect()
    };

    let url = resolve(&cfg.url);
    if url.is_empty() {
        error!("alert: url is empty (env var not set?), dropping alert");
        return;
    }
    let content_type = match cfg.format {
        AlertFormat::CloudEvents => "application/cloudevents+json",
        _ => "application/json",
    };

    for attempt in 0..max_attempts {
        let mut req = client
            .post(&url)
            .header("Content-Type", content_type)
            .json(&body);
        for (k, v) in &cfg.headers {
            req = req.header(k, v);
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                let status = resp.status().as_u16();
                if attempt > 0 {
                    tracing::info!(url = %url, status, attempt, "alert sent (after retry)");
                } else {
                    tracing::info!(url = %url, status, "alert sent");
                }
                return;
            }
            Ok(resp) => {
                let status = resp.status();
                if status.is_server_error() && attempt + 1 < max_attempts {
                    let delay = backoff
                        .get(attempt as usize)
                        .copied()
                        .unwrap_or_else(|| *backoff.last().unwrap());
                    error!(
                        url = %url, %status, attempt,
                        delay_ms = delay.as_millis(),
                        "alert endpoint returned 5xx, will retry"
                    );
                    tokio::time::sleep(delay).await;
                } else {
                    error!(url = %url, %status, attempt, "alert endpoint returned error");
                    return;
                }
            }
            Err(e) => {
                if attempt + 1 < max_attempts {
                    let delay = backoff
                        .get(attempt as usize)
                        .copied()
                        .unwrap_or_else(|| *backoff.last().unwrap());
                    error!(
                        url = %url, error = %e, attempt,
                        delay_ms = delay.as_millis(),
                        "alert send failed, will retry"
                    );
                    tokio::time::sleep(delay).await;
                } else {
                    error!(url = %url, error = %e, attempt, "alert send failed, giving up");
                }
            }
        }
    }
}
