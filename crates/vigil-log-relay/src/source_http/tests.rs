// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use super::*;
use crate::filter::LineFilter;
use tokio::sync::mpsc;

fn passthrough() -> LineFilter {
    LineFilter::from_strs(&[], &[])
}

/// Send `line` through `forward_line` and return what arrived in the channel,
/// with the trailing newline stripped for easier assertions.
fn fwd(line: &str, filter: &LineFilter) -> Option<String> {
    let (tx, mut rx) = mpsc::channel(4);
    forward_line(line.to_owned(), &tx, filter);
    rx.try_recv()
        .ok()
        .map(|s| s.trim_end_matches('\n').to_owned())
}

// --- SSE skips ---

#[test]
fn empty_line_is_skipped() {
    assert_eq!(fwd("", &passthrough()), None);
}

#[test]
fn sse_keepalive_comment_is_skipped() {
    assert_eq!(fwd(": ping", &passthrough()), None);
    assert_eq!(fwd(": keep-alive", &passthrough()), None);
}

#[test]
fn sse_event_field_is_skipped() {
    assert_eq!(fwd("event: heartbeat", &passthrough()), None);
}

#[test]
fn sse_id_field_is_skipped() {
    assert_eq!(fwd("id: 42", &passthrough()), None);
}

#[test]
fn sse_retry_field_is_skipped() {
    assert_eq!(fwd("retry: 3000", &passthrough()), None);
}

// --- SSE data stripping ---

#[test]
fn sse_data_with_space_strips_prefix() {
    let result = fwd(r#"data: {"level":"info","msg":"ok"}"#, &passthrough());
    assert_eq!(result.as_deref(), Some(r#"{"level":"info","msg":"ok"}"#));
}

#[test]
fn sse_data_without_space_strips_prefix() {
    let result = fwd(r#"data:{"level":"info","msg":"ok"}"#, &passthrough());
    assert_eq!(result.as_deref(), Some(r#"{"level":"info","msg":"ok"}"#));
}

#[test]
fn sse_data_empty_payload_is_skipped() {
    assert_eq!(fwd("data:", &passthrough()), None);
    assert_eq!(fwd("data: ", &passthrough()), None);
}

// --- plain ndjson passthrough ---

#[test]
fn plain_ndjson_is_forwarded_verbatim() {
    let line = r#"{"level":"error","msg":"timeout"}"#;
    assert_eq!(fwd(line, &passthrough()).as_deref(), Some(line));
}

// --- filter integration ---

#[test]
fn forward_line_respects_include_filter() {
    let f = LineFilter::from_strs(&["ERROR"], &[]);
    assert!(fwd(r#"{"level":"error"}"#, &f).is_none()); // no "ERROR" in uppercase
    let f2 = LineFilter::from_strs(&["level"], &[]);
    assert!(fwd(r#"{"level":"error"}"#, &f2).is_some());
}

#[test]
fn forward_line_respects_exclude_filter() {
    let f = LineFilter::from_strs(&[], &["healthz"]);
    assert!(fwd(r#"GET /healthz 200"#, &f).is_none());
    assert!(fwd(r#"GET /api/data 200"#, &f).is_some());
}

// --- output has trailing newline ---

#[test]
fn forwarded_line_ends_with_newline() {
    let (tx, mut rx) = mpsc::channel(4);
    forward_line(r#"{"msg":"hi"}"#.to_owned(), &tx, &passthrough());
    let got = rx.try_recv().unwrap();
    assert!(got.ends_with('\n'));
}

// --- bump_failures ---

#[test]
fn bump_failures_increments_counter() {
    assert_eq!(bump_failures(0, 0, "src").unwrap(), 1);
    assert_eq!(bump_failures(4, 0, "src").unwrap(), 5);
}

#[test]
fn bump_failures_unlimited_never_errors() {
    // max_retries = 0 means unlimited
    assert!(bump_failures(9999, 0, "src").is_ok());
}

#[test]
fn bump_failures_exits_at_limit() {
    // errors when failures+1 >= max_retries
    assert!(bump_failures(2, 3, "src").is_err());
}

#[test]
fn bump_failures_below_limit_is_ok() {
    assert!(bump_failures(1, 3, "src").is_ok());
}
