// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors
//
// End-to-end Basic Auth tests: Router → Caller extractor → Handler.
//
// Every test:
//  1. Adds a real identity via the bootstrap window (store is empty → Admin).
//  2. Sends a follow-up request carrying an Authorization: Basic header.
//  3. Asserts the response status according to the caller's access level and
//     the endpoint's required level.
//
// Access ordering: Open < Metrics < Read < Write < Admin

use axum::http::{Method, StatusCode};

use super::{TestApp, body_json};

// ---------------------------------------------------------------------------
// Helper: register a single basic-auth identity and return the app.
// The first POST runs in bootstrap mode (store empty → Admin).
// ---------------------------------------------------------------------------

async fn app_with_basic_identity(name: &str, access: &str, password: &str) -> TestApp {
    use sha_crypt::{Sha512Params, sha512_simple};

    let app = TestApp::new().await;
    let params = Sha512Params::new(1000).unwrap();
    let hash = sha512_simple(password, &params).unwrap();

    app.request(
        Method::POST,
        "/v1/identities",
        Some(serde_json::json!({
            "identities": {
                name: { "access": access, "basic": { "password-hash": hash } }
            }
        })),
    )
    .await;

    app
}

// ---------------------------------------------------------------------------
// Correct credentials — access granted at each level
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_identity_can_access_read_endpoint() {
    let app = app_with_basic_identity("reader", "read", "r3ad").await;

    // GET /v1/services requires Read
    let resp = app
        .request_basic_auth(Method::GET, "/v1/services", "reader", "r3ad", None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

#[tokio::test]
async fn read_identity_can_access_checks_endpoint() {
    let app = app_with_basic_identity("reader", "read", "r3ad").await;

    // GET /v1/checks requires Read
    let resp = app
        .request_basic_auth(Method::GET, "/v1/checks", "reader", "r3ad", None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

#[tokio::test]
async fn write_identity_can_access_write_endpoint() {
    let app = app_with_basic_identity("writer", "write", "wr1te").await;

    // POST /v1/replan requires Write
    let resp = app
        .request_basic_auth(Method::POST, "/v1/replan", "writer", "wr1te", None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

#[tokio::test]
async fn admin_identity_can_access_admin_endpoint() {
    let app = app_with_basic_identity("root", "admin", "adm1n").await;

    // GET /v1/identities requires Admin
    let resp = app
        .request_basic_auth(Method::GET, "/v1/identities", "root", "adm1n", None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body["result"].is_array());
    app.shutdown().await;
}

#[tokio::test]
async fn metrics_identity_can_access_metrics_endpoint() {
    let app = app_with_basic_identity("prom", "metrics", "pr0m").await;

    // GET /v1/metrics requires Metrics
    let resp = app
        .request_basic_auth(Method::GET, "/v1/metrics", "prom", "pr0m", None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// Insufficient access level — 403 returned
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_identity_forbidden_on_write_endpoint() {
    let app = app_with_basic_identity("reader", "read", "r3ad").await;

    // POST /v1/replan requires Write — Read < Write → 403
    let resp = app
        .request_basic_auth(Method::POST, "/v1/replan", "reader", "r3ad", None)
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    app.shutdown().await;
}

#[tokio::test]
async fn read_identity_forbidden_on_admin_endpoint() {
    let app = app_with_basic_identity("reader", "read", "r3ad").await;

    // GET /v1/identities requires Admin — Read < Admin → 403
    let resp = app
        .request_basic_auth(Method::GET, "/v1/identities", "reader", "r3ad", None)
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    app.shutdown().await;
}

#[tokio::test]
async fn metrics_identity_forbidden_on_read_endpoint() {
    let app = app_with_basic_identity("prom", "metrics", "pr0m").await;

    // GET /v1/services requires Read — Metrics < Read → 403
    let resp = app
        .request_basic_auth(Method::GET, "/v1/services", "prom", "pr0m", None)
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    app.shutdown().await;
}

#[tokio::test]
async fn write_identity_forbidden_on_admin_endpoint() {
    let app = app_with_basic_identity("deployer", "write", "wr1te").await;

    // GET /v1/identities requires Admin — Write < Admin → 403
    let resp = app
        .request_basic_auth(Method::GET, "/v1/identities", "deployer", "wr1te", None)
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// Wrong credentials — falls back to Open access → 403 on protected endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wrong_password_falls_back_to_open_and_is_forbidden() {
    let app = app_with_basic_identity("user", "read", "correct").await;

    // Wrong password → no Basic Auth match → Open → Read endpoint returns 403
    let resp = app
        .request_basic_auth(Method::GET, "/v1/services", "user", "wrongpass", None)
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    app.shutdown().await;
}

#[tokio::test]
async fn unknown_user_falls_back_to_open_and_is_forbidden() {
    let app = app_with_basic_identity("known", "read", "pass").await;

    // Unknown username → no match → Open → forbidden
    let resp = app
        .request_basic_auth(Method::GET, "/v1/services", "nobody", "pass", None)
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// Admin identity can access all endpoint levels via Basic Auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_identity_can_access_all_endpoint_levels() {
    let app = app_with_basic_identity("root", "admin", "adm1n").await;

    // Metrics (lowest protected)
    let resp = app
        .request_basic_auth(Method::GET, "/v1/metrics", "root", "adm1n", None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK, "metrics");

    // Read
    let resp = app
        .request_basic_auth(Method::GET, "/v1/services", "root", "adm1n", None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK, "read (services)");

    // Write
    let resp = app
        .request_basic_auth(Method::POST, "/v1/replan", "root", "adm1n", None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK, "write (replan)");

    // Admin
    let resp = app
        .request_basic_auth(Method::GET, "/v1/identities", "root", "adm1n", None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK, "admin (identities)");

    app.shutdown().await;
}
