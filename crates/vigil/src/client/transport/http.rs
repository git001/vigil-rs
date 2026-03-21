// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use vigil_types::api::{LogEntry, Response};

// ---------------------------------------------------------------------------
// HTTP send helpers
// ---------------------------------------------------------------------------

pub async fn http_parse<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> anyhow::Result<T> {
    let status = resp.status();
    let body = resp.bytes().await?;
    let envelope: Response<T> = serde_json::from_slice(&body)
        .map_err(|e| anyhow::anyhow!("invalid response (HTTP {}): {}", status, e))?;
    envelope
        .result
        .ok_or_else(|| anyhow::anyhow!("{}", envelope.message.unwrap_or_default()))
}

pub async fn http_parse_void(resp: reqwest::Response) -> anyhow::Result<()> {
    if resp.status().is_success() {
        return Ok(());
    }
    let status = resp.status();
    let body = resp.bytes().await?;
    let envelope: Response<serde_json::Value> = serde_json::from_slice(&body)
        .map_err(|e| anyhow::anyhow!("invalid response (HTTP {}): {}", status, e))?;
    anyhow::bail!(
        "{}",
        envelope
            .message
            .unwrap_or_else(|| format!("HTTP {}", status))
    )
}

// ---------------------------------------------------------------------------
// SSE helpers
// ---------------------------------------------------------------------------

/// Drain all complete `data:` lines from `buf`, printing each parsed log entry.
pub fn drain_sse_buf(buf: &mut String) {
    while let Some(pos) = buf.find('\n') {
        let line = buf[..pos].trim_end_matches('\r').to_string();
        buf.drain(..=pos);
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if let Ok(entry) = serde_json::from_str::<LogEntry>(data) {
                let stream = format!("{:?}", entry.stream).to_lowercase();
                println!(
                    "{} [{}] [{}] {}",
                    entry.timestamp.format("%Y-%m-%d %H:%M:%S%.3f"),
                    entry.service,
                    stream,
                    entry.message
                );
            }
        }
        // `:` comment keep-alive lines and blank lines are silently ignored.
    }
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

pub fn names_query(names: &[String]) -> String {
    if names.is_empty() {
        String::new()
    } else {
        format!("?names={}", names.join(","))
    }
}
