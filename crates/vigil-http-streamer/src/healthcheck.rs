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
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
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

/// Serve `GET /healthz` on `addr` (e.g. `127.0.0.1:9091`).
/// Returns 200 while liveness is fresh, 503 when stale.
pub async fn serve(addr: String, liveness: Arc<Liveness>) -> Result<()> {
    let listener = TcpListener::bind(&addr).await?;
    info!(addr = %addr, "healthcheck listening on GET /healthz");

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
