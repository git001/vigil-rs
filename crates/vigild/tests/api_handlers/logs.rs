use axum::http::StatusCode;
use serde_json::Value;

use super::{TestApp, body_json};

// ---------------------------------------------------------------------------
// logs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_logs_returns_empty_array() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/logs").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["result"], Value::Array(vec![]));
    app.shutdown().await;
}

#[tokio::test]
async fn get_logs_with_n_param() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/logs?n=10").await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

#[tokio::test]
async fn get_logs_with_service_filter() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/logs?services=foo,bar").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["result"], Value::Array(vec![]));
    app.shutdown().await;
}

#[tokio::test]
async fn follow_logs_default_json_returns_sse() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/logs/follow").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("text/event-stream"), "expected SSE content-type, got: {ct}");
    app.shutdown().await;
}

#[tokio::test]
async fn follow_logs_text_format_returns_sse() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/logs/follow?format=text").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("text/event-stream"), "expected SSE content-type, got: {ct}");
    app.shutdown().await;
}

#[tokio::test]
async fn follow_logs_ndjson_returns_ndjson_content_type() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/logs/follow?format=ndjson").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("application/x-ndjson"), "expected ndjson content-type, got: {ct}");
    app.shutdown().await;
}
