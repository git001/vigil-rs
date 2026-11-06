// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `identities:` in layer files: read-only over the API, replaced by a
//! replan, and they do not end the Unix-socket bootstrap admin.

use axum::http::{Method, StatusCode};

use super::{TestApp, body_json};

/// A read identity for vigil-log-relay, as a ConfigMap would ship it.
/// Password `extrapass` (`openssl passwd -6 -salt testextra extrapass`).
const RELAY_LAYER: &str = r#"identities:
  log-relay:
    access: read
    basic:
      password-hash: "$6$testextra$Cq1GhPxDNtKxM7J.P1EovJ64dyEsjMy59Cr2zHgWhpmUsETC.YtW8GFsrGO810Kh5f/9.SYG53plbhT3VaA7z/"
"#;

#[tokio::test]
async fn layer_identity_authenticates_and_is_listed_with_its_source() {
    let app = TestApp::with_layer(RELAY_LAYER).await;

    let resp = app
        .request_basic_auth(Method::GET, "/v1/services", "log-relay", "extrapass", None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK, "read access");
    let resp = app
        .request_basic_auth(Method::POST, "/v1/replan", "log-relay", "extrapass", None)
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "read only");

    // Bootstrap admin over the socket stays: only API identities end it.
    let resp = app.get("/v1/identities").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let ids = json["result"].as_array().unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0]["name"], "log-relay");
    assert_eq!(ids[0]["source"], "layer");
    assert_eq!(ids[0]["layer"], "001-auth");
    app.shutdown().await;
}

#[tokio::test]
async fn layer_identity_cannot_be_changed_or_removed_over_the_api() {
    let app = TestApp::with_layer(RELAY_LAYER).await;

    let resp = app
        .request(
            Method::POST,
            "/v1/identities",
            Some(serde_json::json!({
                "identities": { "log-relay": { "access": "admin", "local": {} } }
            })),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let json = body_json(resp).await;
    let msg = json["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("identity \"log-relay\" is defined in layer \"001-auth\""),
        "{json}"
    );

    let resp = app
        .request(
            Method::DELETE,
            "/v1/identities",
            Some(serde_json::json!({ "identities": ["log-relay"] })),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // Still there, still read-only.
    let resp = app
        .request_basic_auth(Method::GET, "/v1/services", "log-relay", "extrapass", None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

#[tokio::test]
async fn api_identity_ends_bootstrap_layer_identity_does_not() {
    let app = TestApp::with_layer(RELAY_LAYER).await;
    let resp = app
        .request(
            Method::POST,
            "/v1/identities",
            Some(serde_json::json!({
                "identities": { "ops": { "access": "read", "local": { "user-id": 4_000_000 } } }
            })),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK, "bootstrap admin adds it");
    let resp = app.get("/v1/identities").await;
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "an API identity exists: bootstrap over"
    );
    app.shutdown().await;
}

#[tokio::test]
async fn replan_replaces_the_layer_identities() {
    let app = TestApp::with_layer(RELAY_LAYER).await;
    std::fs::write(app._dir.path().join("001-auth.yaml"), "identities: {}\n").unwrap();
    let resp = app.request(Method::POST, "/v1/replan", None).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .request_basic_auth(Method::GET, "/v1/services", "log-relay", "extrapass", None)
        .await;
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "removed from the layer: access gone at once"
    );
    app.shutdown().await;
}
