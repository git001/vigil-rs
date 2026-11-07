use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use super::{TestApp, body_json};

// ---------------------------------------------------------------------------
// checks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_checks_returns_empty_array() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/checks").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["result"], Value::Array(vec![]));
    app.shutdown().await;
}

#[tokio::test]
async fn list_checks_name_filter_query_param() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/checks?names=foo,bar").await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// POST /v1/checks/refresh
// ---------------------------------------------------------------------------

const REFRESH_LAYER: &str = "checks:
  migrate-done:
    startup: disabled
    exec:
      command: 'echo migration 42 pending; exit 2'
";

#[tokio::test]
async fn refresh_check_returns_info_and_error() {
    let app = TestApp::with_layer(REFRESH_LAYER).await;
    let resp = app
        .request(
            Method::POST,
            "/v1/checks/refresh",
            Some(json!({"name": "migrate-done"})),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(
        body["result"]["error"],
        "exit status 2: migration 42 pending"
    );
    assert_eq!(body["result"]["info"]["name"], "migrate-done");
    assert_eq!(body["result"]["info"]["status"], "down");
    app.shutdown().await;
}

#[tokio::test]
async fn refresh_unknown_check_is_404() {
    let app = TestApp::new().await;
    let resp = app
        .request(
            Method::POST,
            "/v1/checks/refresh",
            Some(json!({"name": "nope"})),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_json(resp).await;
    assert_eq!(body["message"], "check \"nope\" not found");
    app.shutdown().await;
}

#[tokio::test]
async fn refresh_rejects_unknown_fields() {
    let app = TestApp::new().await;
    let resp = app
        .request(
            Method::POST,
            "/v1/checks/refresh",
            Some(json!({"name": "x", "level": "ready"})),
        )
        .await;
    assert!(resp.status().is_client_error(), "{}", resp.status());
    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// GET /v1/health
// ---------------------------------------------------------------------------

const HEALTH_LAYER: &str = "checks:
  web-alive:
    level: alive
    startup: enabled
    delay: 1h
    exec:
      command: 'true'
  db-ready:
    level: ready
    startup: enabled
    delay: 1h
    threshold: 1
    exec:
      command: 'exit 1'
";

#[tokio::test]
async fn health_is_200_then_502_when_a_check_is_down() {
    let app = TestApp::with_layer(HEALTH_LAYER).await;
    let resp = app.get("/v1/health").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["result"]["healthy"], true);
    assert_eq!(body["status-code"], 200);

    // Take db-ready down.
    let resp = app
        .request(
            Method::POST,
            "/v1/checks/refresh",
            Some(json!({"name": "db-ready"})),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app.get("/v1/health?level=ready").await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = body_json(resp).await;
    assert_eq!(body["result"]["healthy"], false);
    assert_eq!(body["status-code"], 502);
    assert_eq!(body["status"], "Bad Gateway");

    // The alive checks are fine.
    let resp = app.get("/v1/health?level=alive").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app.get("/v1/health?names=web-alive").await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

#[tokio::test]
async fn health_unknown_check_is_404() {
    let app = TestApp::with_layer(HEALTH_LAYER).await;
    let resp = app.get("/v1/health?names=web-alvie").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_json(resp).await;
    assert_eq!(body["message"], "check \"web-alvie\" not found");
    app.shutdown().await;
}

#[tokio::test]
async fn health_rejects_unknown_parameters_and_levels() {
    let app = TestApp::with_layer(HEALTH_LAYER).await;
    for uri in ["/v1/health?level=live", "/v1/health?name=web-alive"] {
        let resp = app.get(uri).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{uri}");
    }
    app.shutdown().await;
}

// Open: Kubernetes httpGet probes send no credentials.
#[tokio::test]
async fn health_needs_no_credentials() {
    let app = TestApp::with_layer(HEALTH_LAYER).await;
    let resp = app.request_no_transport(Method::GET, "/v1/health").await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}
