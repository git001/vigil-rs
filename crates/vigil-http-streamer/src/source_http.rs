//! HTTP source modes: URL and Unix-socket.
//!
//! Both functions run a reconnect loop, streaming ndjson lines from the
//! source and forwarding them verbatim to the TCP sink channel.
//! Lines starting with `:` (SSE keep-alive) or empty lines are skipped so
//! the binary works whether the endpoint uses ndjson or SSE framing.
//!
//! # Failure detection and reconnect
//!
//! A reconnect is triggered when any of the following occur:
//!   - Connection refused or timed out
//!   - HTTP non-2xx status code
//!   - Stream read error (broken pipe, reset, …)
//!   - Stream EOF (server closed the connection — clean disconnect)
//!
//! Only connection errors and non-2xx/read errors count toward
//! `--reconnect-retries`. A clean EOF resets the failure counter and the
//! backoff delay, since the remote end closed the connection intentionally
//! (e.g. vigild restarted) and a fast reconnect is desirable.
//!
//! Backoff: starts at `--reconnect-delay` ms, doubles on each consecutive
//! failure, caps at `--reconnect-max` ms.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{bail, Result};
use bytes::Bytes;
use futures::StreamExt as _;
use futures::TryStreamExt;
use http_body_util::BodyStream;
use hyper::Uri;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep};
use tokio_util::io::StreamReader;
use tower::Service;
use tracing::{info, warn};

use crate::Liveness;

// ---------------------------------------------------------------------------
// Reconnect configuration
// ---------------------------------------------------------------------------

pub struct ReconnectConfig {
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    /// 0 = retry forever; N > 0 = exit after N consecutive errors
    pub max_retries: u64,
}

// ---------------------------------------------------------------------------
// URL source (reqwest)
// ---------------------------------------------------------------------------

pub async fn run_url(
    url: String,
    tx: mpsc::Sender<String>,
    liveness: Arc<Liveness>,
    cfg: ReconnectConfig,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true) // allow vigild self-signed TLS
        .build()?;

    // Background ticker: keeps the healthcheck alive even during quiet streams
    // where next_line() blocks indefinitely waiting for log data.
    spawn_liveness_ticker(Arc::clone(&liveness));

    let mut backoff = Duration::from_millis(cfg.initial_delay_ms);
    let mut failures: u64 = 0;

    loop {
        liveness.tick();

        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                info!(url = %url, "connected");
                failures = 0;
                backoff = Duration::from_millis(cfg.initial_delay_ms);

                let byte_stream = resp
                    .bytes_stream()
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
                let mut lines = BufReader::new(StreamReader::new(byte_stream)).lines();

                let mut clean_eof = true;
                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) => forward_line(line, &tx),
                        Ok(None) => {
                            info!(url = %url, "stream EOF — reconnecting");
                            break;
                        }
                        Err(e) => {
                            warn!(url = %url, error = %e, "read error — reconnecting");
                            clean_eof = false;
                            break;
                        }
                    }
                }
                if !clean_eof {
                    failures = bump_failures(failures, cfg.max_retries, &url)?;
                }
            }
            Ok(resp) => {
                warn!(url = %url, status = %resp.status(), "unexpected HTTP status");
                failures = bump_failures(failures, cfg.max_retries, &url)?;
            }
            Err(e) => {
                warn!(url = %url, error = %e, backoff_ms = backoff.as_millis(), "connection failed");
                failures = bump_failures(failures, cfg.max_retries, &url)?;
            }
        }

        sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_millis(cfg.max_delay_ms));
    }
}

// ---------------------------------------------------------------------------
// Unix-socket source (hyper)
// ---------------------------------------------------------------------------

pub async fn run_unix(
    socket: PathBuf,
    path: String,
    tx: mpsc::Sender<String>,
    liveness: Arc<Liveness>,
    cfg: ReconnectConfig,
) -> Result<()> {
    let connector = UnixConnector(Arc::new(socket.clone()));
    let client: Client<UnixConnector, http_body_util::Empty<Bytes>> =
        Client::builder(TokioExecutor::new()).build(connector);

    spawn_liveness_ticker(Arc::clone(&liveness));

    let mut backoff = Duration::from_millis(cfg.initial_delay_ms);
    let mut failures: u64 = 0;

    loop {
        liveness.tick();

        let uri: Uri = format!("http://localhost{}", path).parse()?;
        let req = hyper::Request::get(uri).body(http_body_util::Empty::<Bytes>::new())?;

        match client.request(req).await {
            Ok(resp) if resp.status().is_success() => {
                info!(socket = %socket.display(), path = %path, "connected");
                failures = 0;
                backoff = Duration::from_millis(cfg.initial_delay_ms);

                // .boxed() erases the type to satisfy StreamReader's Unpin bound
                let byte_stream = BodyStream::new(resp.into_body())
                    .try_filter_map(|frame| async { Ok(frame.into_data().ok()) })
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
                    .boxed();
                let mut lines = BufReader::new(StreamReader::new(byte_stream)).lines();

                let mut clean_eof = true;
                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) => forward_line(line, &tx),
                        Ok(None) => {
                            info!(socket = %socket.display(), "stream EOF — reconnecting");
                            break;
                        }
                        Err(e) => {
                            warn!(socket = %socket.display(), error = %e, "read error — reconnecting");
                            clean_eof = false;
                            break;
                        }
                    }
                }
                if !clean_eof {
                    failures = bump_failures(failures, cfg.max_retries, socket.to_str().unwrap_or("socket"))?;
                }
            }
            Ok(resp) => {
                warn!(socket = %socket.display(), status = %resp.status(), "unexpected HTTP status");
                failures = bump_failures(failures, cfg.max_retries, socket.to_str().unwrap_or("socket"))?;
            }
            Err(e) => {
                warn!(socket = %socket.display(), error = %e, backoff_ms = backoff.as_millis(), "connection failed");
                failures = bump_failures(failures, cfg.max_retries, socket.to_str().unwrap_or("socket"))?;
            }
        }

        sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_millis(cfg.max_delay_ms));
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Forward one ndjson line to the TCP sink channel.
/// Skips empty lines and SSE keep-alive comments (`: ping`).
fn forward_line(line: String, tx: &mpsc::Sender<String>) {
    if line.is_empty() || line.starts_with(':') {
        return;
    }
    let mut line = line;
    line.push('\n');
    if tx.try_send(line).is_err() {
        warn!("send buffer full — dropping log line");
    }
}

/// Increment the failure counter; bail if max_retries exceeded.
fn bump_failures(failures: u64, max_retries: u64, source: &str) -> Result<u64> {
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
fn spawn_liveness_ticker(liveness: Arc<Liveness>) {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(30));
        loop {
            ticker.tick().await;
            liveness.tick();
        }
    });
}

// ---------------------------------------------------------------------------
// Minimal Unix-socket connector for hyper
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct UnixConnector(Arc<PathBuf>);

impl Service<Uri> for UnixConnector {
    type Response = TokioIo<UnixStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let path = Arc::clone(&self.0);
        Box::pin(async move {
            let stream = UnixStream::connect(path.as_path()).await?;
            Ok(TokioIo::new(stream))
        })
    }
}
