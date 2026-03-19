//! Shared types and helpers for HTTP-based source modes.
//!
//! Both ndjson and SSE framing are handled transparently in `forward_line`.
//!
//! | Input line            | Action                                    |
//! |-----------------------|-------------------------------------------|
//! | *(empty)*             | skip (SSE event delimiter)                |
//! | `: …`                 | skip (SSE keepalive / comment)            |
//! | `event: …`            | skip (SSE event type — not forwarded)     |
//! | `id: …`               | skip (SSE event id — not forwarded)       |
//! | `retry: …`            | skip (SSE retry hint — not forwarded)     |
//! | `data: <payload>`     | strip prefix, forward `<payload>`         |
//! | `data:<payload>`      | strip prefix, forward `<payload>`         |
//! | anything else         | forward verbatim (plain ndjson line)      |

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::warn;

use crate::{LineFilter, Liveness};

// ---------------------------------------------------------------------------
// Configuration types (re-exported from crate root via main.rs)
// ---------------------------------------------------------------------------

pub struct ReconnectConfig {
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    /// 0 = retry forever; N > 0 = exit after N consecutive errors
    pub max_retries: u64,
}

pub struct SourceConnConfig {
    /// TCP connect timeout in ms (0 = no timeout).
    pub connect_timeout_ms: u64,
    /// Max time without new data before reconnect in ms (0 = disabled).
    pub read_timeout_ms: u64,
    /// Per-line idle timeout in ms; reconnects if no line arrives (0 = disabled).
    pub idle_timeout_ms: u64,
    /// TCP keepalive interval in seconds (0 = disabled).
    pub keepalive_interval_secs: u64,
    /// TCP keepalive probe timeout in seconds (0 = OS default).
    pub keepalive_timeout_secs: u64,
    /// Explicit proxy URL (overrides HTTP_PROXY / HTTPS_PROXY env vars).
    pub proxy_url: Option<String>,
    /// Skip TLS certificate verification for the proxy connection.
    pub proxy_insecure: bool,
    /// PEM file with one or more CA certificates (chain) used to verify the proxy's TLS.
    pub proxy_cacert: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Shared helpers (used by source_url and source_unix)
// ---------------------------------------------------------------------------

/// Forward one line to the TCP sink channel, handling both ndjson and SSE framing.
///
/// SSE metadata lines and keepalives are dropped. `data:` prefixes are stripped
/// so the output is always plain ndjson regardless of source framing.
pub fn forward_line(line: String, tx: &mpsc::Sender<String>, filter: &LineFilter) {
    if line.is_empty() || line.starts_with(':') {
        return;
    }
    if line.starts_with("event:") || line.starts_with("id:") || line.starts_with("retry:") {
        return;
    }
    let payload = if let Some(rest) = line.strip_prefix("data:") {
        rest.trim_start_matches(' ').to_owned()
    } else {
        line
    };
    if payload.is_empty() {
        return;
    }
    if !filter.allow(&payload) {
        return;
    }
    let mut payload = payload;
    payload.push('\n');
    if tx.try_send(payload).is_err() {
        warn!("send buffer full — dropping log line");
    }
}

/// Increment the failure counter; bail if max_retries exceeded.
pub fn bump_failures(failures: u64, max_retries: u64, source: &str) -> Result<u64> {
    let n = failures + 1;
    if max_retries > 0 && n >= max_retries {
        bail!(
            "source '{}' exceeded {} consecutive reconnect failures — exiting so \
             vigild can restart with its own backoff policy",
            source,
            max_retries
        );
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;
    use super::*;
    use crate::filter::LineFilter;

    fn passthrough() -> LineFilter {
        LineFilter::from_strs(&[], &[])
    }

    /// Send `line` through `forward_line` and return what arrived in the channel,
    /// with the trailing newline stripped for easier assertions.
    fn fwd(line: &str, filter: &LineFilter) -> Option<String> {
        let (tx, mut rx) = mpsc::channel(4);
        forward_line(line.to_owned(), &tx, filter);
        rx.try_recv().ok().map(|s| s.trim_end_matches('\n').to_owned())
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
}

/// Spawn a background task that ticks liveness every 30 s.
/// This keeps the healthcheck alive even when the stream is idle
/// (next_line() blocks waiting for data from a quiet source).
pub fn spawn_liveness_ticker(liveness: Arc<Liveness>) {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(30));
        loop {
            ticker.tick().await;
            liveness.tick();
        }
    });
}
