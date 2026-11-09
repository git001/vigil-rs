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
    sse_payload(line.to_owned()).filter(|p| filter.allow(p))
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
    queue_payload(r#"{"msg":"hi"}"#.to_owned(), &tx);
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

#[tokio::test]
async fn stream_loop_survives_invalid_utf8_and_splits_too_long_lines() {
    let mut input = b"{\"a\":1}\n\xff{\"b\":2}\n".to_vec();
    input.extend(std::iter::repeat_n(b'x', 12000));
    input.extend(b"\n{\"c\":3}\n");
    let (tx, mut rx) = mpsc::channel(8);
    // --max-line-bytes 1024: records up to 6 × 1024 + 4096 = 10240 bytes.
    let conn = SourceConnConfig {
        join: crate::join::JoinConfig {
            max: 1024,
            multiline: None,
        },
        ..Default::default()
    };
    let clean_eof = stream_loop(&input[..], &tx, &passthrough(), &conn, "test").await;
    assert!(clean_eof);
    drop(tx);
    let mut got = Vec::new();
    while let Some(line) = rx.recv().await {
        got.push(line);
    }
    let parts = ["x".repeat(10240) + "\n", "x".repeat(1760) + "\n"];
    assert_eq!(
        got,
        [
            "{\"a\":1}\n",
            "\u{fffd}{\"b\":2}\n",
            &parts[0],
            &parts[1],
            "{\"c\":3}\n"
        ]
    );
}

#[tokio::test]
async fn stream_loop_joins_stack_traces_per_service() {
    let rec = |svc: &str, msg: &str| {
        format!(
            "{{\"service\":\"{svc}\",\"stream\":\"stdout\",\"message\":{}}}\n",
            serde_json::to_string(msg).unwrap()
        )
    };
    let input = [
        rec("api", "ERROR failed"),
        rec("web", "GET /"),
        rec("api", "java.lang.IllegalStateException: boom"),
        rec("api", "\tat a.B.c(B.java:1)"),
        rec("api", "next"),
    ]
    .concat();
    let conn = SourceConnConfig {
        join: crate::join::JoinConfig {
            max: 1024,
            multiline: Some((
                vigil_types::lines::Rule::Preset(vigil_types::plan::MultilinePreset::Java),
                std::time::Duration::from_secs(10),
            )),
        },
        ..Default::default()
    };
    let (tx, mut rx) = mpsc::channel(8);
    assert!(stream_loop(input.as_bytes(), &tx, &passthrough(), &conn, "test").await);
    drop(tx);
    let mut got = Vec::new();
    while let Some(line) = rx.recv().await {
        got.push(line);
    }
    // At the end of the stream the pending entries are sent too.
    got.sort();
    let mut want = vec![
        rec(
            "api",
            "ERROR failed\njava.lang.IllegalStateException: boom\n\tat a.B.c(B.java:1)",
        ),
        rec("api", "next"),
        rec("web", "GET /"),
    ];
    want.sort();
    assert_eq!(got, want);
}
