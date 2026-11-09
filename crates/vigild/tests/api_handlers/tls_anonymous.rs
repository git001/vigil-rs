// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! S5: callers without a matching identity.
//!
//! Before v4 every caller was admin while no identity existed, including
//! anonymous callers on the TLS listener. Now TLS callers get at most
//! `--tls-anonymous-access` (open|metrics); only the Unix socket keeps the
//! bootstrap admin, and only for root and vigild's own UID.

use axum::http::{Method, StatusCode};
use vigil_types::identity::IdentityAccess;

use super::TestApp;

#[tokio::test]
async fn tls_anonymous_default_metrics_allows_probes_and_metrics_only() {
    let app = TestApp::new().await; // no identities, default `metrics`

    for uri in ["/v1/checks", "/v1/health", "/v1/system-info", "/v1/metrics"] {
        let resp = app.request_tls(Method::GET, uri, None).await;
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
    }
    for uri in [
        "/v1/services",
        "/v1/logs",
        "/v1/logs/follow",
        "/v1/alerts",
        "/v1/identities",
        "/v1/plan",
    ] {
        let resp = app.request_tls(Method::GET, uri, None).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{uri}");
    }
    let resp = app.request_tls(Method::POST, "/v1/replan", None).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "replan");
    let resp = app
        .request_tls(
            Method::POST,
            "/v1/identities",
            Some(serde_json::json!({
                "identities": { "evil": { "access": "admin", "local": {} } }
            })),
        )
        .await;
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "anonymous TLS caller must not bootstrap identities"
    );
    app.shutdown().await;
}

#[tokio::test]
async fn tls_anonymous_open_hides_metrics() {
    let app = TestApp::with_tls_anonymous(IdentityAccess::Open).await;
    let resp = app.request_tls(Method::GET, "/v1/metrics", None).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    for uri in ["/v1/checks", "/v1/health"] {
        let resp = app.request_tls(Method::GET, uri, None).await;
        assert_eq!(resp.status(), StatusCode::OK, "probes keep working: {uri}");
    }
    app.shutdown().await;
}

#[tokio::test]
async fn unix_own_uid_is_admin_while_no_identities() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/identities").await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

#[tokio::test]
async fn caller_without_transport_info_gets_open() {
    let app = TestApp::new().await;
    let resp = app.request_no_transport(Method::GET, "/v1/services").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let resp = app.request_no_transport(Method::GET, "/v1/checks").await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

#[tokio::test]
async fn checks_stay_open_after_identities_exist() {
    let app = TestApp::new().await;
    let resp = app
        .request(
            Method::POST,
            "/v1/identities",
            Some(serde_json::json!({
                "identities": { "ops": { "access": "admin", "basic": { "password-hash": "$6$x$y" } } }
            })),
        )
        .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "identity added, bootstrap over"
    );
    let resp = app.request_tls(Method::GET, "/v1/checks", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app.request_tls(Method::GET, "/v1/services", None).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    app.shutdown().await;
}
