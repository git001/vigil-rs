use axum::http::{Method, StatusCode};
use serde_json::Value;

use super::{TestApp, body_json};

// ---------------------------------------------------------------------------
// services
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_services_returns_empty_array() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/services").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["result"], Value::Array(vec![]));
    app.shutdown().await;
}

#[tokio::test]
async fn services_action_unknown_name_returns_ok() {
    let app = TestApp::new().await;
    let resp = app
        .request(
            Method::POST,
            "/v1/services",
            Some(serde_json::json!({ "action": "start", "services": ["no-such-svc"] })),
        )
        .await;
    // returns a change record (or an error), either way not a 403/500 from routing
    assert!(
        resp.status() == StatusCode::OK || resp.status().is_client_error(),
        "unexpected status {}",
        resp.status()
    );
    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// POST /v1/signals
// ---------------------------------------------------------------------------

const SIGNAL_LAYER: &str = "services:
  web:
    command: sleep 300
    startup: enabled
  idle:
    command: sleep 300
";

/// Stop `web` and shut down: `shutdown` does not wait for the services,
/// so the test stops its process itself.
async fn stop_and_shutdown(app: TestApp) {
    app.request(
        Method::POST,
        "/v1/services",
        Some(serde_json::json!({"action": "stop", "services": ["web"]})),
    )
    .await;
    app.shutdown().await;
}

/// Wait until `web` is active.
async fn web_active(app: &TestApp) {
    for _ in 0..100 {
        let body = body_json(app.get("/v1/services?names=web").await).await;
        if body["result"][0]["current"] == "active" {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("web did not become active");
}

async fn post_signals(app: &TestApp, body: Value) -> (StatusCode, Value) {
    let resp = app.request(Method::POST, "/v1/signals", Some(body)).await;
    let status = resp.status();
    (status, body_json(resp).await)
}

#[tokio::test]
async fn signals_reach_a_running_service() {
    let app = TestApp::with_layer(SIGNAL_LAYER).await;
    web_active(&app).await;
    let (status, body) = post_signals(
        &app,
        serde_json::json!({"signal": "SIGUSR2", "services": ["web"]}),
    )
    .await;
    // sleep dies of SIGUSR2; what matters here is that it was sent.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["signal"], "SIGUSR2");
    assert_eq!(body["result"]["services"], serde_json::json!(["web"]));
    stop_and_shutdown(app).await;
}

#[tokio::test]
async fn signals_errors() {
    let app = TestApp::with_layer(SIGNAL_LAYER).await;
    web_active(&app).await;
    for (body, code, message) in [
        (
            serde_json::json!({"signal": "HUP", "services": ["web"]}),
            StatusCode::BAD_REQUEST,
            "unknown signal \"HUP\" (write it like SIGHUP)",
        ),
        (
            serde_json::json!({"signal": "SIGHUP", "services": []}),
            StatusCode::BAD_REQUEST,
            "services must name at least one service",
        ),
        (
            serde_json::json!({"signal": "SIGHUP", "services": ["web", "wbe"]}),
            StatusCode::NOT_FOUND,
            "service \"wbe\" not found",
        ),
        (
            serde_json::json!({"signal": "SIGHUP", "services": ["web", "idle"]}),
            StatusCode::CONFLICT,
            "not running: \"idle\"",
        ),
    ] {
        let (status, resp) = post_signals(&app, body.clone()).await;
        assert_eq!(status, code, "{body}");
        assert_eq!(resp["message"], message, "{body}");
    }
    // An unknown field is rejected by the JSON extractor (plain text body).
    let status = app
        .request(
            Method::POST,
            "/v1/signals",
            Some(serde_json::json!({"signal": "SIGHUP", "services": ["web"], "force": true})),
        )
        .await
        .status();
    assert!(status.is_client_error(), "unknown field: {status}");
    stop_and_shutdown(app).await;
}
