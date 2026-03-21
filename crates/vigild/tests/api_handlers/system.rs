use axum::http::StatusCode;

use super::{TestApp, body_json};

// ---------------------------------------------------------------------------
// system-info
// ---------------------------------------------------------------------------

#[tokio::test]
async fn system_info_returns_200() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/system-info").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["status"], "OK");
    assert!(body["result"]["version"].is_string());
    assert!(body["result"]["boot-id"].is_string());
    assert!(body["result"]["start-time"].is_string());
    app.shutdown().await;
}
