// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! vigild HTTP client — public API surface.

mod http_client;
mod logs;
mod transport;

pub use http_client::HttpConfig;

use std::path::PathBuf;

use http_client::build_reqwest_client;
use hyper::Method;
use transport::{Transport, names_query, parse, parse_void};
use vigil_types::api::{
    AlertInfo, ChangeInfo, CheckInfo, CheckRefresh, CheckRefreshRequest, DaemonAction,
    DaemonActionRequest, ServiceAction, ServiceInfo, ServicesAction, SignalsRequest, SignalsResult,
    SystemInfo,
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
    ///
    /// The daemon must run as root or as our own euid; see
    /// [`VigilClient::new_unix_trusting`] for daemons running as another UID.
    pub fn new_unix(socket_path: PathBuf) -> Self {
        Self::new_unix_trusting(socket_path, None)
    }

    /// Like [`VigilClient::new_unix`], but additionally trusts a daemon
    /// running as `trusted_uid` (e.g. a non-root vigild that grants this user
    /// access through a local identity).
    pub fn new_unix_trusting(socket_path: PathBuf, trusted_uid: Option<u32>) -> Self {
        VigilClient {
            transport: Transport::new_unix(socket_path, trusted_uid),
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
        let (status, body) = self.transport.send(Method::GET, path, None).await?;
        parse(status, &body)
    }

    async fn post<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        let (status, body) = self.send_json(Method::POST, path, body).await?;
        parse(status, &body)
    }

    /// POST to an endpoint that returns no meaningful body (`ApiResult<()>`).
    async fn post_void<B: serde::Serialize>(&self, path: &str, body: &B) -> anyhow::Result<()> {
        let (status, body) = self.send_json(Method::POST, path, body).await?;
        parse_void(status, &body)
    }

    async fn delete<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        let (status, body) = self.send_json(Method::DELETE, path, body).await?;
        parse(status, &body)
    }

    async fn send_json<B: serde::Serialize>(
        &self,
        method: Method,
        path: &str,
        body: &B,
    ) -> anyhow::Result<(hyper::StatusCode, bytes::Bytes)> {
        let json = serde_json::to_vec(body)?;
        self.transport.send(method, path, Some(json)).await
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

    /// Send a signal to services (`POST /v1/signals`).
    pub async fn send_signals(
        &self,
        signal: &str,
        services: Vec<String>,
    ) -> anyhow::Result<SignalsResult> {
        self.post(
            "/v1/signals",
            &SignalsRequest {
                signal: signal.to_owned(),
                services,
            },
        )
        .await
    }

    /// Run a check now (`POST /v1/checks/refresh`).
    pub async fn refresh_check(&self, name: &str) -> anyhow::Result<CheckRefresh> {
        self.post(
            "/v1/checks/refresh",
            &CheckRefreshRequest {
                name: name.to_owned(),
            },
        )
        .await
    }

    pub async fn list_alerts(&self, names: &[String]) -> anyhow::Result<Vec<AlertInfo>> {
        self.get(&format!("/v1/alerts{}", names_query(names))).await
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
