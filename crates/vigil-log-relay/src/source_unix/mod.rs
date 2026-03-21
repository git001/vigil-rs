//! Unix-domain socket source — connects via hyper over a local Unix socket.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use futures::{StreamExt as _, TryStreamExt};
use http_body_util::BodyStream;
use hyper::Uri;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::io::StreamReader;
use tracing::{info, warn};

use crate::source_http::{bump_failures, spawn_liveness_ticker, stream_loop};
use crate::{LineFilter, Liveness, ReconnectConfig, SourceConnConfig};

use self::connector::UnixConnector;

// ---------------------------------------------------------------------------
// Unix socket source entry point
// ---------------------------------------------------------------------------

pub async fn run(
    socket: PathBuf,
    path: String,
    tx: mpsc::Sender<String>,
    liveness: Arc<Liveness>,
    conn: SourceConnConfig,
    cfg: ReconnectConfig,
    filter: LineFilter,
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

                let byte_stream = BodyStream::new(resp.into_body())
                    .try_filter_map(|frame| async { Ok(frame.into_data().ok()) })
                    .map_err(std::io::Error::other)
                    .boxed();
                let lines = BufReader::new(StreamReader::new(byte_stream)).lines();

                let source_label = socket.to_str().unwrap_or("socket");
                let clean_eof =
                    stream_loop(lines, &tx, &filter, conn.idle_timeout_ms, source_label).await;
                if !clean_eof {
                    failures = bump_failures(failures, cfg.max_retries, source_label)?;
                }
            }
            Ok(resp) => {
                warn!(socket = %socket.display(), status = %resp.status(), "unexpected HTTP status");
                failures = bump_failures(
                    failures,
                    cfg.max_retries,
                    socket.to_str().unwrap_or("socket"),
                )?;
            }
            Err(e) => {
                warn!(socket = %socket.display(), error = %e, backoff_ms = backoff.as_millis(), "connection failed");
                failures = bump_failures(
                    failures,
                    cfg.max_retries,
                    socket.to_str().unwrap_or("socket"),
                )?;
            }
        }

        sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_millis(cfg.max_delay_ms));
    }
}

mod connector;
#[cfg(test)]
mod tests;
