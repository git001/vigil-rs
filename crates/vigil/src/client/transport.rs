// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Low-level transport layer: Unix-socket connector and HTTP/Unix send helpers.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Uri;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::UnixStream;
use tower::Service;
use vigil_types::api::{LogEntry, Response};

// ---------------------------------------------------------------------------
// Unix socket connector (hyper)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(super) struct UnixConnector(pub(super) Arc<PathBuf>);

impl Service<Uri> for UnixConnector {
    type Response = TokioIo<UnixStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let path = Arc::clone(&self.0);
        Box::pin(async move {
            let stream = UnixStream::connect(path.as_path()).await?;
            Ok(TokioIo::new(stream))
        })
    }
}

// ---------------------------------------------------------------------------
// Transport enum
// ---------------------------------------------------------------------------

pub(super) enum Transport {
    Unix(Client<UnixConnector, Full<Bytes>>),
    Http { client: reqwest::Client, base_url: String },
}

impl Transport {
    pub(super) fn new_unix(path: PathBuf) -> Self {
        let connector = UnixConnector(Arc::new(path));
        Transport::Unix(Client::builder(TokioExecutor::new()).build(connector))
    }
}

// ---------------------------------------------------------------------------
// Unix send helpers
// ---------------------------------------------------------------------------

pub(super) fn unix_uri(path: &str) -> anyhow::Result<Uri> {
    Ok(format!("http://localhost{}", path).parse()?)
}

pub(super) async fn unix_send<T: serde::de::DeserializeOwned>(
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
pub(super) async fn unix_send_void(
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
    anyhow::bail!("{}", envelope.message.unwrap_or_else(|| format!("HTTP {}", status)))
}

// ---------------------------------------------------------------------------
// HTTP send helpers
// ---------------------------------------------------------------------------

pub(super) async fn http_parse<T: serde::de::DeserializeOwned>(
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

pub(super) async fn http_parse_void(resp: reqwest::Response) -> anyhow::Result<()> {
    if resp.status().is_success() {
        return Ok(());
    }
    let status = resp.status();
    let body = resp.bytes().await?;
    let envelope: Response<serde_json::Value> = serde_json::from_slice(&body)
        .map_err(|e| anyhow::anyhow!("invalid response (HTTP {}): {}", status, e))?;
    anyhow::bail!("{}", envelope.message.unwrap_or_else(|| format!("HTTP {}", status)))
}

// ---------------------------------------------------------------------------
// SSE helpers
// ---------------------------------------------------------------------------

/// Drain all complete `data:` lines from `buf`, printing each parsed log entry.
pub(super) fn drain_sse_buf(buf: &mut String) {
    while let Some(pos) = buf.find('\n') {
        let line = buf[..pos].trim_end_matches('\r').to_string();
        buf.drain(..=pos);
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if let Ok(entry) = serde_json::from_str::<LogEntry>(data) {
                let stream = format!("{:?}", entry.stream).to_lowercase();
                println!(
                    "{} [{}] [{}] {}",
                    entry.timestamp.format("%H:%M:%S%.3f"),
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

pub(super) fn names_query(names: &[String]) -> String {
    if names.is_empty() {
        String::new()
    } else {
        format!("?names={}", names.join(","))
    }
}
