// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use super::*;

// -----------------------------------------------------------------------
// drain_sse_buf tests
// -----------------------------------------------------------------------

/// Empty buffer: nothing to drain, buffer stays empty.
#[test]
fn drain_sse_buf_empty_buffer() {
    let mut buf = String::new();
    drain_sse_buf(&mut buf);
    assert!(buf.is_empty(), "empty buffer should remain empty after drain");
}

/// Partial data without a newline: buffer must be left unchanged.
#[test]
fn drain_sse_buf_no_newline_yet() {
    let mut buf = "data:{\"partial".to_string();
    drain_sse_buf(&mut buf);
    assert_eq!(
        buf, "data:{\"partial",
        "incomplete line (no newline) must not be consumed"
    );
}

/// A SSE comment keep-alive line (`:keepalive\n`) should be consumed
/// without panicking and leave the buffer empty.
#[test]
fn drain_sse_buf_comment_line_skipped() {
    let mut buf = ":keepalive\n".to_string();
    drain_sse_buf(&mut buf);
    assert!(
        buf.is_empty(),
        "comment line should be consumed and buffer should be empty"
    );
}

/// A line without a `data:` prefix should be consumed silently.
#[test]
fn drain_sse_buf_non_data_line_skipped() {
    let mut buf = "event:update\n".to_string();
    drain_sse_buf(&mut buf);
    assert!(
        buf.is_empty(),
        "non-data line should be consumed and buffer should be empty"
    );
}

/// A `data:` line with invalid JSON should be consumed without panicking.
#[test]
fn drain_sse_buf_invalid_json_skipped() {
    let mut buf = "data:notjson\n".to_string();
    drain_sse_buf(&mut buf);
    assert!(
        buf.is_empty(),
        "invalid-JSON data line should be silently consumed"
    );
}

/// A valid `data:` line with a well-formed `LogEntry` JSON must be
/// consumed from the buffer (the function prints it to stdout).
#[test]
fn drain_sse_buf_valid_data_line() {
    let json = r#"{"timestamp":"2026-01-01T00:00:00Z","service":"svc","stream":"stdout","message":"hello"}"#;
    let mut buf = format!("data:{}\n", json);
    drain_sse_buf(&mut buf);
    assert!(
        buf.is_empty(),
        "valid data line should be fully consumed from the buffer"
    );
}

/// Multiple lines: comments, blank, non-data, and a valid entry — all
/// complete lines are consumed, only a trailing partial line is retained.
#[test]
fn drain_sse_buf_multiple_lines() {
    let json = r#"{"timestamp":"2026-01-01T00:00:00Z","service":"svc","stream":"stderr","message":"err"}"#;
    let mut buf = format!(
        ":keepalive\nevent:ping\ndata:{}\npartial",
        json
    );
    drain_sse_buf(&mut buf);
    assert_eq!(
        buf, "partial",
        "all complete lines should be drained; trailing partial line must be retained"
    );
}

/// Simulates receiving data in two chunks: first an incomplete line (no
/// drain), then the rest completing it (drained).
#[test]
fn drain_sse_buf_partial_then_complete() {
    let mut buf = String::new();

    // First chunk: incomplete — no newline yet.
    buf.push_str(r#"data:{"timestamp":"2026-01-01T00:00:00Z","service":"s","stream":"stdout","message":"m"}"#);
    drain_sse_buf(&mut buf);
    assert!(
        !buf.is_empty(),
        "incomplete line must not be consumed on first drain"
    );

    // Second chunk: newline arrives — line is now complete.
    buf.push('\n');
    drain_sse_buf(&mut buf);
    assert!(
        buf.is_empty(),
        "completed line should be fully consumed on second drain"
    );
}

/// CRLF line endings (`\r\n`) must be handled: the `\r` is stripped and
/// the line is consumed correctly.
#[test]
fn drain_sse_buf_crlf_line_ending() {
    let json = r#"{"timestamp":"2026-01-01T00:00:00Z","service":"svc","stream":"stdout","message":"crlf"}"#;
    let mut buf = format!("data:{}\r\n", json);
    drain_sse_buf(&mut buf);
    assert!(
        buf.is_empty(),
        "CRLF-terminated data line should be fully consumed"
    );
}

// -----------------------------------------------------------------------
// names_query tests
// -----------------------------------------------------------------------

#[test]
fn names_query_empty() {
    assert_eq!(
        names_query(&[]),
        "",
        "empty slice should produce an empty string"
    );
}

#[test]
fn names_query_single() {
    let names = vec!["foo".to_string()];
    assert_eq!(names_query(&names), "?names=foo");
}

#[test]
fn names_query_multiple() {
    let names = vec!["a".to_string(), "b".to_string()];
    assert_eq!(names_query(&names), "?names=a,b");
}

// -----------------------------------------------------------------------
// unix_uri tests
// -----------------------------------------------------------------------

#[test]
fn unix_uri_builds_http_localhost_uri() {
    let uri = unix_uri("/v1/services").expect("unix_uri should not fail for a valid path");
    assert_eq!(uri.to_string(), "http://localhost/v1/services");
}

#[test]
fn unix_uri_with_query() {
    let uri = unix_uri("/v1/services?names=foo").unwrap();
    assert_eq!(uri.to_string(), "http://localhost/v1/services?names=foo");
}

// -----------------------------------------------------------------------
// http_parse / http_parse_void tests
// -----------------------------------------------------------------------

/// Build a fake `reqwest::Response` from an `http::Response` (via hyper's
/// re-export) so we can test `http_parse` / `http_parse_void` without a
/// live server.  `reqwest::Response` implements `From<http::Response<T>>`
/// when the body type implements `Into<reqwest::Body>`.
fn make_resp(status: u16, body: &'static str) -> reqwest::Response {
    let raw = hyper::http::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(body)
        .unwrap();
    reqwest::Response::from(raw)
}

#[tokio::test]
async fn http_parse_success() {
    let resp = make_resp(200, r#"{"type":"sync","status-code":200,"status":"OK","result":42}"#);
    let val: u32 = http_parse(resp).await.expect("http_parse should succeed");
    assert_eq!(val, 42);
}

#[tokio::test]
async fn http_parse_error_message() {
    let resp = make_resp(
        400,
        r#"{"type":"error","status-code":400,"status":"Bad Request","result":null,"message":"bad input"}"#,
    );
    let err = http_parse::<u32>(resp)
        .await
        .expect_err("should return Err for null result");
    assert!(
        err.to_string().contains("bad input"),
        "error message should propagate: {err}"
    );
}

#[tokio::test]
async fn http_parse_invalid_json_error() {
    let resp = make_resp(200, "not json at all");
    let err = http_parse::<u32>(resp)
        .await
        .expect_err("malformed JSON should return Err");
    assert!(
        err.to_string().contains("invalid response"),
        "error should mention 'invalid response': {err}"
    );
}

#[tokio::test]
async fn http_parse_void_success() {
    let resp = make_resp(200, "");
    http_parse_void(resp)
        .await
        .expect("http_parse_void should return Ok for 2xx");
}

#[tokio::test]
async fn http_parse_void_error_message() {
    let resp = make_resp(
        500,
        r#"{"type":"error","status-code":500,"status":"Internal Server Error","result":null,"message":"server exploded"}"#,
    );
    let err = http_parse_void(resp)
        .await
        .expect_err("non-2xx should return Err");
    assert!(
        err.to_string().contains("server exploded"),
        "error message should propagate: {err}"
    );
}

#[tokio::test]
async fn http_parse_void_invalid_json_error() {
    let resp = make_resp(503, "garbage");
    let err = http_parse_void(resp)
        .await
        .expect_err("malformed JSON on non-2xx should return Err");
    assert!(
        err.to_string().contains("invalid response"),
        "error should mention 'invalid response': {err}"
    );
}

#[tokio::test]
async fn http_parse_void_fallback_http_status_message() {
    // result is null, message is None → falls back to "HTTP 400"
    let resp = make_resp(
        400,
        r#"{"type":"error","status-code":400,"status":"Bad Request","result":null}"#,
    );
    let err = http_parse_void(resp)
        .await
        .expect_err("should return Err");
    assert!(
        err.to_string().contains("400"),
        "fallback message should include HTTP status: {err}"
    );
}
