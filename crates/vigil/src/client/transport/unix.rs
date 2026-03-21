// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::Client;
use vigil_types::api::Response;

use super::connector::UnixConnector;

// ---------------------------------------------------------------------------
// Unix send helpers
// ---------------------------------------------------------------------------

pub fn unix_uri(path: &str) -> anyhow::Result<hyper::Uri> {
    Ok(format!("http://localhost{}", path).parse()?)
}

pub async fn unix_send<T: serde::de::DeserializeOwned>(
    client: &Client<UnixConnector, Full<Bytes>>,
    req: hyper::Request<Full<Bytes>>,
) -> anyhow::Result<T> {
    let resp = client.request(req).await?;
    let status = resp.status();
    let body = resp.into_body().collect().await?.to_bytes();
    let envelope: Response<T> = serde_json::from_slice(&body)
        .map_err(|e| anyhow::anyhow!("invalid response (HTTP {}): {}", status, e))?;
    envelope
        .result
        .ok_or_else(|| anyhow::anyhow!("{}", envelope.message.unwrap_or_default()))
}

/// Like `unix_send` but for void endpoints — checks HTTP status instead of
/// deserializing (`serde_json` maps `null` → `None` for `Option<()>`).
pub async fn unix_send_void(
    client: &Client<UnixConnector, Full<Bytes>>,
    req: hyper::Request<Full<Bytes>>,
) -> anyhow::Result<()> {
    let resp = client.request(req).await?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.into_body().collect().await?.to_bytes();
    let envelope: Response<serde_json::Value> = serde_json::from_slice(&body)
        .map_err(|e| anyhow::anyhow!("invalid response (HTTP {}): {}", status, e))?;
    anyhow::bail!(
        "{}",
        envelope
            .message
            .unwrap_or_else(|| format!("HTTP {}", status))
    )
}
