use std::time::Duration;

use axum::http::StatusCode;
use http_body_util::BodyExt;
use serde_json::Value;
use vigil_types::api::{LogEntry, LogStream};

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
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        ct.contains("text/event-stream"),
        "expected SSE content-type, got: {ct}"
    );
    app.shutdown().await;
}

#[tokio::test]
async fn follow_logs_text_format_returns_sse() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/logs/follow?format=text").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        ct.contains("text/event-stream"),
        "expected SSE content-type, got: {ct}"
    );
    app.shutdown().await;
}

#[tokio::test]
async fn follow_logs_ndjson_returns_ndjson_content_type() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/logs/follow?format=ndjson").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        ct.contains("application/x-ndjson"),
        "expected ndjson content-type, got: {ct}"
    );
    app.shutdown().await;
}

#[tokio::test]
async fn get_logs_rejects_unknown_query_parameters() {
    let app = TestApp::new().await;
    // `format` belongs to /v1/logs/follow, not to /v1/logs.
    let resp = app.get("/v1/logs?format=ndjson").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    let msg = body["message"].as_str().unwrap();
    assert!(msg.contains("unknown field `format`"), "{msg}");
    // Not a boolean.
    let resp = app.get("/v1/logs?follow=yes").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    app.shutdown().await;
}

fn entry(service: &str, message: &str) -> LogEntry {
    LogEntry {
        timestamp: chrono::Utc::now(),
        service: service.into(),
        stream: LogStream::Stdout,
        message: message.into(),
    }
}

/// Reads the next chunk of a streamed body, as text.
async fn next_chunk(body: &mut axum::body::Body) -> String {
    let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
        .await
        .expect("no log line within 5s")
        .expect("stream ended")
        .unwrap();
    String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap()
}

#[tokio::test]
async fn get_logs_follow_streams_the_tail_then_new_lines_as_ndjson() {
    let app = TestApp::new().await;
    app.log_store.push(entry("web", "old line")).await;
    app.log_store.push(entry("web", "last line")).await;
    app.log_store.push(entry("worker", "other service")).await;

    let resp = app.get("/v1/logs?follow=true&services=web&n=1").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()["content-type"].to_str().unwrap(),
        "application/x-ndjson"
    );
    let mut body = resp.into_body();

    // The tail: only the last line of web.
    let line = next_chunk(&mut body).await;
    assert!(line.ends_with('\n'), "{line:?}");
    let first: Value = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(first["service"], "web");
    assert_eq!(first["message"], "last line");

    // Then new lines; worker is filtered out.
    app.log_store.push(entry("worker", "not for us")).await;
    app.log_store.push(entry("web", "new line")).await;
    let line = next_chunk(&mut body).await;
    let next: Value = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(next["message"], "new line");
    app.shutdown().await;
}
