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
use tokio::io::AsyncRead;
use tokio::io::BufReader;
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{info, warn};

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

/// Drive the inner streaming loop shared by `source_url` and `source_unix`.
///
/// Reads lines from `lines` until EOF, a read error, or an idle-timeout
/// expiry.  Returns `true` on a clean EOF and `false` on any unclean exit
/// (error or timeout), so the caller knows whether to increment the failure
/// counter.
///
/// * `idle_timeout_ms` — per-line timeout in milliseconds; 0 disables it.
/// * `source_label` — human-readable name used in log messages
///   (e.g. the URL string or socket path).
pub async fn stream_loop<R>(
    mut lines: tokio::io::Lines<BufReader<R>>,
    tx: &mpsc::Sender<String>,
    filter: &LineFilter,
    idle_timeout_ms: u64,
    source_label: &str,
) -> bool
where
    R: AsyncRead + Unpin,
{
    loop {
        let next = if idle_timeout_ms > 0 {
            match tokio::time::timeout(Duration::from_millis(idle_timeout_ms), lines.next_line())
                .await
            {
                Ok(r) => r,
                Err(_) => {
                    warn!(source = %source_label, "idle timeout — reconnecting");
                    return false;
                }
            }
        } else {
            lines.next_line().await
        };

        match next {
            Ok(Some(line)) => forward_line(line, tx, filter),
            Ok(None) => {
                info!(source = %source_label, "stream EOF — reconnecting");
                return true;
            }
            Err(e) => {
                warn!(source = %source_label, error = %e, "read error — reconnecting");
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests;
