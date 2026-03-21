use axum::http::{Method, StatusCode};

use super::{TestApp, body_json};

// ---------------------------------------------------------------------------
// metrics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_metrics_returns_openmetrics_content() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/metrics").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("openmetrics"),
        "content-type: {content_type}"
    );
    app.shutdown().await;
}

#[tokio::test]
async fn metrics_body_contains_all_family_names() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/metrics").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body();
    use http_body_util::BodyExt as _;
    let body = String::from_utf8(bytes.collect().await.unwrap().to_bytes().to_vec()).unwrap();

    for name in &[
        "vigil_service_start_count",
        "vigil_service_active",
        "vigil_service_info",
        "vigil_services_count",
        "vigil_check_up",
        "vigil_check_success_count",
        "vigil_check_failure_count",
        "vigil_alert_fire_count",
    ] {
        assert!(
            body.contains(name),
            "metric '{name}' missing from /v1/metrics response"
        );
    }
    assert!(body.contains("# EOF"), "OpenMetrics EOF marker missing");
    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// replan
// ---------------------------------------------------------------------------

#[tokio::test]
async fn replan_returns_ok() {
    let app = TestApp::new().await;
    let resp = app.request(Method::POST, "/v1/replan", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// daemon action
// ---------------------------------------------------------------------------

#[tokio::test]
async fn daemon_stop_returns_ok() {
    let app = TestApp::new().await;
    let resp = app
        .request(
            Method::POST,
            "/v1/vigild",
            Some(serde_json::json!({ "action": "stop" })),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// changes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_unknown_change_returns_error() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/changes/no-such-id").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// OpenAPI
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openapi_json_is_served() {
    let app = TestApp::new().await;
    let resp = app.get("/openapi.json").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["info"]["title"], "vigil API");
    app.shutdown().await;
}
