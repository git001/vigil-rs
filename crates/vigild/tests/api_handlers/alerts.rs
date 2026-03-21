// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::http::{Method, StatusCode};
use serde_json::Value;

use super::{TestApp, body_json};

// ---------------------------------------------------------------------------
// alerts — basic smoke test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_alerts_returns_empty_array() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/alerts").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["result"], Value::Array(vec![]));
    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// alerts — access control
// GET /v1/alerts requires Read.  After bootstrap ends, unauthenticated callers
// get Open access and must receive 403.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_alerts_forbidden_without_credentials_after_bootstrap() {
    let app = TestApp::new().await;

    // End bootstrap mode by registering an identity.
    app.request(
        Method::POST,
        "/v1/identities",
        Some(serde_json::json!({
            "identities": { "op": { "access": "admin", "local": {} } }
        })),
    )
    .await;

    // Unauthenticated GET /v1/alerts — Open < Read → 403
    let resp = app.get("/v1/alerts").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    app.shutdown().await;
}

#[tokio::test]
async fn list_alerts_accessible_with_read_basic_auth() {
    use sha_crypt::{Sha512Params, sha512_simple};

    let app = TestApp::new().await;
    let params = Sha512Params::new(1000).unwrap();
    let hash = sha512_simple("readpass", &params).unwrap();

    app.request(
        Method::POST,
        "/v1/identities",
        Some(serde_json::json!({
            "identities": {
                "monitor": { "access": "read", "basic": { "password-hash": hash } }
            }
        })),
    )
    .await;

    let resp = app
        .request_basic_auth(Method::GET, "/v1/alerts", "monitor", "readpass", None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body["result"].is_array(), "expected result array");
    app.shutdown().await;
}

#[tokio::test]
async fn list_alerts_forbidden_with_metrics_basic_auth() {
    use sha_crypt::{Sha512Params, sha512_simple};

    let app = TestApp::new().await;
    let params = Sha512Params::new(1000).unwrap();
    let hash = sha512_simple("prompass", &params).unwrap();

    app.request(
        Method::POST,
        "/v1/identities",
        Some(serde_json::json!({
            "identities": {
                "prom": { "access": "metrics", "basic": { "password-hash": hash } }
            }
        })),
    )
    .await;

    // Metrics < Read → 403
    let resp = app
        .request_basic_auth(Method::GET, "/v1/alerts", "prom", "prompass", None)
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    app.shutdown().await;
}
