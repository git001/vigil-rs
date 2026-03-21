// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors
//
// Integration tests for the vigild HTTP API handlers.
//
// Each test builds a real in-process AppState (overlord + log store +
// identity store + metrics) and drives the axum router via
// `tower::ServiceExt::oneshot` — no network port is bound.
//
// Auth note: when the identity store is empty the auth extractor grants
// `Admin` access automatically (bootstrap mode), so no credentials are
// needed in these tests.

use axum::body::Body;
use axum::http::{Method, Request};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use http_body_util::BodyExt as _;
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tower::ServiceExt;

use vigild::api::{AppState, router};
use vigild::identity::IdentityStore;
use vigild::overlord;
use vigild::server::TlsPeerCert;
// nix is a direct dep of vigild — available transitively in integration tests
extern crate nix;

mod alerts;
mod auth;
mod checks;
mod identities;
mod logs;
mod misc;
mod services;
mod system;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

pub struct TestApp {
    pub router: axum::Router,
    /// Keep TempDir alive so the layers directory exists for the overlord.
    pub _dir: TempDir,
    /// Keep the overlord tx so we can shut it down cleanly.
    pub overlord_tx: mpsc::Sender<overlord::Cmd>,
}

impl TestApp {
    pub async fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let (handle, log_store, metrics, _task) =
            overlord::spawn(dir.path().to_owned(), "test-addr".into(), None, 100).unwrap();
        let identity_store = IdentityStore::new();
        let (shutdown_tx, _shutdown_rx) = mpsc::channel(1);

        let state = AppState {
            overlord: handle.clone(),
            log_store,
            identity_store,
            metrics,
            shutdown_tx,
        };

        TestApp {
            router: router(state),
            _dir: dir,
            overlord_tx: handle.tx,
        }
    }

    /// Send a GET request and return the response.
    pub async fn get(&self, uri: &str) -> axum::response::Response {
        self.router
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    /// Send a request with an arbitrary method/body and return the response.
    pub async fn request(
        &self,
        method: Method,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> axum::response::Response {
        self.request_inner(method, uri, body, false).await
    }

    /// Same as `request`, but injects a `ConnectInfo<UnixPeerInfo>` extension
    /// so that local-identity auth resolves for the current process UID.
    pub async fn request_auth(
        &self,
        method: Method,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> axum::response::Response {
        self.request_inner(method, uri, body, true).await
    }

    async fn request_inner(
        &self,
        method: Method,
        uri: &str,
        body: Option<serde_json::Value>,
        with_local_auth: bool,
    ) -> axum::response::Response {
        use axum::extract::ConnectInfo;
        use vigild::server::UnixPeerInfo;

        let mut builder = Request::builder().method(method).uri(uri);
        if with_local_auth {
            let uid = nix::unistd::Uid::effective().as_raw();
            builder = builder.extension(ConnectInfo(UnixPeerInfo { uid: Some(uid) }));
        }
        let http_body = if let Some(json) = body {
            builder = builder.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&json).unwrap())
        } else {
            Body::empty()
        };
        self.router
            .clone()
            .oneshot(builder.body(http_body).unwrap())
            .await
            .unwrap()
    }

    /// Send a GET or other request with an `Authorization: Basic` header.
    pub async fn request_basic_auth(
        &self,
        method: Method,
        uri: &str,
        user: &str,
        pass: &str,
        body: Option<serde_json::Value>,
    ) -> axum::response::Response {
        let encoded = B64.encode(format!("{user}:{pass}"));
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", format!("Basic {encoded}"));
        let http_body = if let Some(json) = body {
            builder = builder.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&json).unwrap())
        } else {
            Body::empty()
        };
        self.router
            .clone()
            .oneshot(builder.body(http_body).unwrap())
            .await
            .unwrap()
    }

    /// Send a request as if the TLS handshake presented `cert_der` as the
    /// client certificate (injects a `TlsPeerCert` extension).
    pub async fn request_mtls(
        &self,
        method: Method,
        uri: &str,
        cert_der: Vec<u8>,
        body: Option<serde_json::Value>,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .extension(TlsPeerCert(cert_der));
        let http_body = if let Some(json) = body {
            builder = builder.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&json).unwrap())
        } else {
            Body::empty()
        };
        self.router
            .clone()
            .oneshot(builder.body(http_body).unwrap())
            .await
            .unwrap()
    }

    pub async fn shutdown(self) {
        let _ = self.overlord_tx.send(overlord::Cmd::Shutdown).await;
    }
}

/// Consume the response body and parse it as JSON.
pub async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}
