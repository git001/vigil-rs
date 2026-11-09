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

use anyhow::{Result, bail};
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::warn;

use crate::Liveness;

// ---------------------------------------------------------------------------
// Configuration types (re-exported from crate root via main.rs)
// ---------------------------------------------------------------------------

pub struct ReconnectConfig {
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    /// 0 = retry forever; N > 0 = exit after N consecutive errors
    pub max_retries: u64,
}

#[derive(Default)]
pub struct SourceConnConfig {
    /// TCP connect timeout in ms (0 = no timeout).
    pub connect_timeout_ms: u64,
    /// Max time without new data before reconnect in ms (0 = disabled).
    pub read_timeout_ms: u64,
    /// Per-line idle timeout in ms; reconnects if no line arrives (0 = disabled).
    pub idle_timeout_ms: u64,
    /// Longest message and multi-line rule (`--max-line-bytes`,
    /// `--multiline*`); a record may be longer, see
    /// [`crate::cli::record_limit`].
    pub join: crate::join::JoinConfig,
    /// TCP keepalive (see [`crate::keepalive`]).
    pub keepalive: crate::keepalive::Keepalive,
    /// Skip TLS certificate verification for the source connection.
    pub source_insecure: bool,
    /// PEM file with one or more CA certificates (chain) used to verify the source's TLS.
    pub source_cacert: Option<PathBuf>,
    /// Explicit proxy URL (overrides HTTP_PROXY / HTTPS_PROXY env vars).
    pub proxy_url: Option<String>,
    /// Skip TLS certificate verification for the proxy connection.
    pub proxy_insecure: bool,
    /// PEM file with one or more CA certificates (chain) used to verify the proxy's TLS.
    pub proxy_cacert: Option<PathBuf>,
    /// Comma-separated list of hosts / CIDRs that bypass the proxy.
    pub no_proxy: Option<String>,
    /// File with `user:password` for HTTP Basic Auth (re-read per connect).
    pub source_user_file: Option<PathBuf>,
    /// PEM client certificate for mTLS (together with `source_key`).
    pub source_cert: Option<PathBuf>,
    /// PEM private key for `source_cert`.
    pub source_key: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Shared helpers (used by source_url and source_unix)
// ---------------------------------------------------------------------------

/// The payload of a line, handling both ndjson and SSE framing: `None` for
/// SSE metadata, keepalives and empty lines, the rest without a `data:`
/// prefix, so the output is plain ndjson regardless of source framing.
pub fn sse_payload(line: String) -> Option<String> {
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    if line.starts_with("event:") || line.starts_with("id:") || line.starts_with("retry:") {
        return None;
    }
    let payload = match line.strip_prefix("data:") {
        Some(rest) => rest.trim_start_matches(' ').to_owned(),
        None => line,
    };
    (!payload.is_empty()).then_some(payload)
}

/// Send one payload to the sink channel, as one line.
pub fn queue_payload(mut payload: String, tx: &mpsc::Sender<String>) {
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

mod keyed;
mod stream;

pub use stream::stream_loop;

#[cfg(test)]
mod tests;
