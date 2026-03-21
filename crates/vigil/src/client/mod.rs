// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! vigild HTTP client — public API surface.

mod http_client;
mod transport;

pub use http_client::HttpConfig;

use std::path::PathBuf;

use bytes::Bytes;
use http_body_util::Full;
use http_client::build_reqwest_client;
use transport::{
    Transport, drain_sse_buf, http_parse, http_parse_void, names_query, unix_send, unix_send_void,
    unix_uri,
};
use vigil_types::api::{
    AlertInfo, ChangeInfo, CheckInfo, DaemonAction, DaemonActionRequest, LogEntry, ServiceAction,
    ServiceInfo, ServicesAction, SystemInfo,
};
use vigil_types::identity::{
    AddIdentitiesRequest, Identity, IdentitySpec, RemoveIdentitiesRequest,
};

// ---------------------------------------------------------------------------
// VigilClient
// ---------------------------------------------------------------------------

pub struct VigilClient {
    transport: Transport,
}

impl VigilClient {
    /// Connect via Unix domain socket (default transport).
    pub fn new_unix(socket_path: PathBuf) -> Self {
        VigilClient {
            transport: Transport::new_unix(socket_path),
        }
    }

    /// Connect via HTTP or HTTPS URL with full proxy and TLS configuration.
    pub fn new_http(base_url: String, config: HttpConfig) -> anyhow::Result<Self> {
        let client = build_reqwest_client(config)?;
        let base_url = base_url.trim_end_matches('/').to_string();
        Ok(VigilClient {
            transport: Transport::Http { client, base_url },
        })
    }

    // -----------------------------------------------------------------------
    // Low-level request helpers
    // -----------------------------------------------------------------------

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        match &self.transport {
            Transport::Unix(c) => {
                let req = hyper::Request::builder()
                    .method("GET")
                    .uri(unix_uri(path)?)
                    .body(Full::default())?;
                unix_send(c, req).await
            }
            Transport::Http { client, base_url } => {
                let resp = client.get(format!("{}{}", base_url, path)).send().await?;
                http_parse(resp).await
            }
        }
    }

    async fn post<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        match &self.transport {
            Transport::Unix(c) => {
                let bytes = Bytes::from(serde_json::to_vec(body)?);
                let req = hyper::Request::builder()
                    .method("POST")
                    .uri(unix_uri(path)?)
                    .header("content-type", "application/json")
                    .body(Full::new(bytes))?;
                unix_send(c, req).await
            }
            Transport::Http { client, base_url } => {
                let resp = client
                    .post(format!("{}{}", base_url, path))
                    .json(body)
                    .send()
                    .await?;
                http_parse(resp).await
            }
        }
    }

    /// POST to an endpoint that returns no meaningful body (`ApiResult<()>`).
    async fn post_void<B: serde::Serialize>(&self, path: &str, body: &B) -> anyhow::Result<()> {
        match &self.transport {
            Transport::Unix(c) => {
                let bytes = Bytes::from(serde_json::to_vec(body)?);
                let req = hyper::Request::builder()
                    .method("POST")
                    .uri(unix_uri(path)?)
                    .header("content-type", "application/json")
                    .body(Full::new(bytes))?;
                unix_send_void(c, req).await
            }
            Transport::Http { client, base_url } => {
                let resp = client
                    .post(format!("{}{}", base_url, path))
                    .json(body)
                    .send()
                    .await?;
                http_parse_void(resp).await
            }
        }
    }

    async fn delete<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        match &self.transport {
            Transport::Unix(c) => {
                let bytes = Bytes::from(serde_json::to_vec(body)?);
                let req = hyper::Request::builder()
                    .method("DELETE")
                    .uri(unix_uri(path)?)
                    .header("content-type", "application/json")
                    .body(Full::new(bytes))?;
                unix_send(c, req).await
            }
            Transport::Http { client, base_url } => {
                let resp = client
                    .delete(format!("{}{}", base_url, path))
                    .json(body)
                    .send()
                    .await?;
                http_parse(resp).await
            }
        }
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    pub async fn system_info(&self) -> anyhow::Result<SystemInfo> {
        self.get("/v1/system-info").await
    }

    pub async fn list_services(&self, names: &[String]) -> anyhow::Result<Vec<ServiceInfo>> {
        self.get(&format!("/v1/services{}", names_query(names)))
            .await
    }

    pub async fn services_action(
        &self,
        action: ServiceAction,
        services: Vec<String>,
    ) -> anyhow::Result<ChangeInfo> {
        self.post("/v1/services", &ServicesAction { action, services })
            .await
    }

    pub async fn list_checks(&self, names: &[String]) -> anyhow::Result<Vec<CheckInfo>> {
        self.get(&format!("/v1/checks{}", names_query(names))).await
    }

    pub async fn list_alerts(&self, names: &[String]) -> anyhow::Result<Vec<AlertInfo>> {
        self.get(&format!("/v1/alerts{}", names_query(names))).await
    }

    pub async fn list_logs(
        &self,
        services: &[String],
        n: Option<usize>,
    ) -> anyhow::Result<Vec<LogEntry>> {
        let mut params = Vec::new();
        if !services.is_empty() {
            params.push(format!("services={}", services.join(",")));
        }
        if let Some(n) = n {
            params.push(format!("n={}", n));
        }
        let query = if params.is_empty() {
            String::new()
        } else {
            format!("?{}", params.join("&"))
        };
        self.get(&format!("/v1/logs{}", query)).await
    }

    /// Stream live log entries (SSE) — prints to stdout until EOF or Ctrl-C.
    pub async fn follow_logs(&self, services: &[String]) -> anyhow::Result<()> {
        let path = if services.is_empty() {
            "/v1/logs/follow".to_string()
        } else {
            format!("/v1/logs/follow?services={}", services.join(","))
        };

        match &self.transport {
            Transport::Unix(client) => {
                let req = hyper::Request::builder()
                    .method("GET")
                    .uri(unix_uri(&path)?)
                    .header("accept", "text/event-stream")
                    .body(Full::default())?;

                let resp = client.request(req).await?;
                let status = resp.status();
                if !status.is_success() {
                    let body = resp.into_body();
                    use http_body_util::BodyExt as _;
                    let bytes = body.collect().await?.to_bytes();
                    anyhow::bail!("HTTP {}: {}", status, String::from_utf8_lossy(&bytes));
                }

                let mut body = resp.into_body();
                let mut buf = String::new();
                loop {
                    use http_body_util::BodyExt as _;
                    match body.frame().await {
                        Some(Ok(frame)) => {
                            if let Some(chunk) = frame.data_ref() {
                                buf.push_str(&String::from_utf8_lossy(chunk));
                            }
                            drain_sse_buf(&mut buf);
                        }
                        Some(Err(e)) => return Err(e.into()),
                        None => break,
                    }
                }
            }

            Transport::Http { client, base_url } => {
                let resp = client
                    .get(format!("{}{}", base_url, path))
                    .header("accept", "text/event-stream")
                    .send()
                    .await?;
                let status = resp.status();
                if !status.is_success() {
                    anyhow::bail!("HTTP {}: {}", status, resp.text().await?);
                }
                let mut resp = resp;
                let mut buf = String::new();
                while let Some(chunk) = resp.chunk().await? {
                    buf.push_str(&String::from_utf8_lossy(&chunk));
                    drain_sse_buf(&mut buf);
                }
            }
        }

        Ok(())
    }

    pub async fn replan(&self) -> anyhow::Result<()> {
        self.post_void("/v1/replan", &serde_json::Value::Null).await
    }

    pub async fn daemon_action(&self, action: DaemonAction) -> anyhow::Result<()> {
        self.post_void("/v1/vigild", &DaemonActionRequest { action })
            .await
    }

    pub async fn list_identities(&self, names: &[String]) -> anyhow::Result<Vec<Identity>> {
        self.get(&format!("/v1/identities{}", names_query(names)))
            .await
    }

    pub async fn add_identities(
        &self,
        identities: std::collections::HashMap<String, IdentitySpec>,
    ) -> anyhow::Result<()> {
        self.post_void("/v1/identities", &AddIdentitiesRequest { identities })
            .await
    }

    pub async fn remove_identities(&self, names: Vec<String>) -> anyhow::Result<Vec<String>> {
        self.delete(
            "/v1/identities",
            &RemoveIdentitiesRequest { identities: names },
        )
        .await
    }
}
