use axum::http::StatusCode;
use serde_json::Value;

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
