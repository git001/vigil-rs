//! Unix-domain socket source — connects via hyper over a local Unix socket.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use futures::{StreamExt as _, TryStreamExt};
use http_body_util::BodyStream;
use hyper::Uri;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::AsyncBufReadExt;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::io::StreamReader;
use tower::Service;
use tracing::{info, warn};

use crate::{LineFilter, Liveness, ReconnectConfig, SourceConnConfig};
use crate::source_http::{bump_failures, forward_line, spawn_liveness_ticker};

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
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
                    .boxed();
                let mut lines = tokio::io::BufReader::new(StreamReader::new(byte_stream)).lines();

                let mut clean_eof = true;
                loop {
                    let next = if conn.idle_timeout_ms > 0 {
                        match tokio::time::timeout(
                            Duration::from_millis(conn.idle_timeout_ms),
                            lines.next_line(),
                        ).await {
                            Ok(r) => r,
                            Err(_) => {
                                warn!(socket = %socket.display(), "idle timeout — reconnecting");
                                clean_eof = false;
                                break;
                            }
                        }
                    } else {
                        lines.next_line().await
                    };
                    match next {
                        Ok(Some(line)) => forward_line(line, &tx, &filter),
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
