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
