// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Sending a request over the Unix socket or HTTP(S).

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Method, StatusCode};

use super::Transport;

const JSON: &str = "application/json";

/// The URI of `path` on the Unix socket (the host is ignored).
pub fn unix_uri(path: &str) -> anyhow::Result<hyper::Uri> {
    Ok(format!("http://localhost{}", path).parse()?)
}

impl Transport {
    /// Send a request with an optional JSON body; returns the status and
    /// the whole body.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        json: Option<Vec<u8>>,
    ) -> anyhow::Result<(StatusCode, Bytes)> {
        match self {
            Transport::Unix(client) => {
                let mut req = hyper::Request::builder()
                    .method(method)
                    .uri(unix_uri(path)?);
                let body = match json {
                    Some(bytes) => {
                        req = req.header("content-type", JSON);
                        Full::new(Bytes::from(bytes))
                    }
                    None => Full::default(),
                };
                let resp = client.request(req.body(body)?).await?;
                let status = resp.status();
                Ok((status, resp.into_body().collect().await?.to_bytes()))
            }
            Transport::Http { client, base_url } => {
                let mut req = client.request(method, format!("{}{}", base_url, path));
                if let Some(bytes) = json {
                    req = req.header("content-type", JSON).body(bytes);
                }
                let resp = req.send().await?;
                let status = resp.status();
                Ok((status, resp.bytes().await?))
            }
        }
    }

    /// GET `path` and hand each chunk of the body to `on_chunk` until the
    /// server ends the stream. A status other than 2xx is an error with the
    /// body.
    pub async fn stream(
        &self,
        path: &str,
        accept: &str,
        mut on_chunk: impl FnMut(&[u8]),
    ) -> anyhow::Result<()> {
        match self {
            Transport::Unix(client) => {
                let req = hyper::Request::builder()
                    .method(Method::GET)
                    .uri(unix_uri(path)?)
                    .header("accept", accept)
                    .body(Full::default())?;
                let resp = client.request(req).await?;
                let status = resp.status();
                let mut body = resp.into_body();
                if !status.is_success() {
                    let bytes = body.collect().await?.to_bytes();
                    anyhow::bail!("HTTP {}: {}", status, String::from_utf8_lossy(&bytes));
                }
                while let Some(frame) = body.frame().await {
                    if let Some(chunk) = frame?.data_ref() {
                        on_chunk(chunk);
                    }
                }
            }
            Transport::Http { client, base_url } => {
                let mut resp = client
                    .get(format!("{}{}", base_url, path))
                    .header("accept", accept)
                    .send()
                    .await?;
                let status = resp.status();
                if !status.is_success() {
                    anyhow::bail!("HTTP {}: {}", status, resp.text().await?);
                }
                while let Some(chunk) = resp.chunk().await? {
                    on_chunk(&chunk);
                }
            }
        }
        Ok(())
    }
}
