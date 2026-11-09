//! HTTP health check server.
//!
//! Exposes `GET /healthz` on a configurable address.
//! Returns 200 OK while the source loop is active, 503 Service Unavailable
//! if no tick has been received within `max_age_secs`.
//!
//! Sources call `Liveness::tick()` to signal they are still making progress:
//!   - `source_k8s`: tick at the end of each watch cycle.
//!   - `source_http`: background ticker (30 s) + tick on each reconnect attempt.

use std::convert::Infallible;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tracing::info;

// ---------------------------------------------------------------------------
// Liveness tracker
// ---------------------------------------------------------------------------

pub struct Liveness {
    last_tick: AtomicU64,
    max_age_secs: u64,
}

impl Liveness {
    pub fn new(max_age_secs: u64) -> Arc<Self> {
        Arc::new(Self {
            last_tick: AtomicU64::new(now_secs()),
            max_age_secs,
        })
    }

    /// Called by source loops to signal progress.
    pub fn tick(&self) {
        self.last_tick.store(now_secs(), Ordering::Relaxed);
    }

    fn is_alive(&self) -> bool {
        now_secs().saturating_sub(self.last_tick.load(Ordering::Relaxed)) <= self.max_age_secs
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------------------------------------------------------------------------
// HTTP server
// ---------------------------------------------------------------------------

/// Bind the healthcheck address (e.g. `127.0.0.1:9091`) at startup, so a
/// bind error is fatal. Port `0` lets the kernel pick a free port; the log
/// line shows the one actually bound.
pub async fn bind(addr: &str) -> Result<TcpListener> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding healthcheck address {addr}"))?;
    info!(addr = %listener.local_addr()?, "healthcheck listening on GET /healthz");
    Ok(listener)
}

/// Serve `GET /healthz` on a listener from [`bind`].
/// Returns 200 while liveness is fresh, 503 when stale.
pub async fn serve(listener: TcpListener, liveness: Arc<Liveness>) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let liveness = Arc::clone(&liveness);
        tokio::spawn(async move {
            let svc = service_fn(move |_req: Request<Incoming>| {
                let liveness = Arc::clone(&liveness);
                async move {
                    let (status, body) = if liveness.is_alive() {
                        (StatusCode::OK, "ok\n")
                    } else {
                        (StatusCode::SERVICE_UNAVAILABLE, "stale\n")
                    };
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(status)
                            .body(Full::new(Bytes::from(body)))
                            .unwrap(),
                    )
                }
            });
            http1::Builder::new()
                .serve_connection(TokioIo::new(stream), svc)
                .await
                .ok();
        });
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// Status line of `GET /healthz` against `serve` with `liveness`.
    async fn status(liveness: Arc<Liveness>) -> String {
        let listener = bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(serve(listener, liveness));
        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut resp = String::new();
        conn.read_to_string(&mut resp).await.unwrap();
        server.abort();
        resp.lines().next().unwrap_or_default().to_string()
    }

    #[tokio::test]
    async fn healthz_is_ok_while_fresh_and_503_when_stale() {
        let fresh = Liveness::new(30);
        assert_eq!(status(Arc::clone(&fresh)).await, "HTTP/1.1 200 OK");

        // No tick for longer than max age: stale.
        let stale = Liveness::new(30);
        stale.last_tick.store(now_secs() - 31, Ordering::Relaxed);
        assert_eq!(
            status(Arc::clone(&stale)).await,
            "HTTP/1.1 503 Service Unavailable"
        );
        // A tick makes it fresh again.
        stale.tick();
        assert_eq!(status(stale).await, "HTTP/1.1 200 OK");
    }
}
