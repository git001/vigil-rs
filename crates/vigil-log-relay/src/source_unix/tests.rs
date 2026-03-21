use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

use super::run;
use crate::{LineFilter, Liveness, ReconnectConfig, SourceConnConfig};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Unique temp socket path per test (pid + counter avoids stale-socket conflicts).
fn tmp_socket() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let p =
        std::env::temp_dir().join(format!("vigil_test_unix_{}_{}.sock", std::process::id(), n));
    let _ = std::fs::remove_file(&p); // remove stale socket if any
    p
}

fn passthrough() -> LineFilter {
    LineFilter::from_strs(&[], &[])
}

fn default_conn() -> SourceConnConfig {
    SourceConnConfig {
        connect_timeout_ms: 500,
        read_timeout_ms: 0,
        idle_timeout_ms: 0,
        keepalive_interval_secs: 0,
        keepalive_timeout_secs: 0,
        source_insecure: false,
        source_cacert: None,
        proxy_url: None,
        proxy_insecure: false,
        proxy_cacert: None,
        no_proxy: None,
    }
}

fn fast_retry(max_retries: u64) -> ReconnectConfig {
    ReconnectConfig {
        initial_delay_ms: 1,
        max_delay_ms: 10,
        max_retries,
    }
}

/// Bind a Unix socket, accept connections in a loop, read the HTTP request,
/// write `response`, then hold the connection open for `hold_ms` ms.
async fn start_unix_mock(path: &std::path::Path, response: &'static str, hold_ms: u64) {
    let listener = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        while let Ok((mut conn, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let _ = conn.read(&mut buf).await;
                let _ = conn.write_all(response.as_bytes()).await;
                if hold_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(hold_ms)).await;
                }
            });
        }
    });
}

// -----------------------------------------------------------------------

#[tokio::test]
async fn connect_error_exits_after_max_retries() {
    let path = tmp_socket(); // does not exist → ENOENT on connect

    let (tx, _rx) = mpsc::channel(8);
    let err = tokio::time::timeout(
        Duration::from_secs(5),
        run(
            path,
            "/v1/logs/follow".to_owned(),
            tx,
            Liveness::new(60),
            default_conn(),
            fast_retry(1),
            passthrough(),
        ),
    )
    .await
    .expect("timed out")
    .unwrap_err();

    assert!(
        err.to_string().contains("consecutive reconnect failures"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn non_2xx_exits_after_max_retries() {
    let path = tmp_socket();
    start_unix_mock(
        &path,
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
        0,
    )
    .await;

    let (tx, _rx) = mpsc::channel(8);
    let err = tokio::time::timeout(
        Duration::from_secs(5),
        run(
            path,
            "/v1/logs/follow".to_owned(),
            tx,
            Liveness::new(60),
            default_conn(),
            fast_retry(1),
            passthrough(),
        ),
    )
    .await
    .expect("timed out")
    .unwrap_err();

    assert!(
        err.to_string().contains("consecutive reconnect failures"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn stream_lines_are_forwarded() {
    let path = tmp_socket();
    let listener = UnixListener::bind(&path).unwrap();

    tokio::spawn(async move {
        // Round 1: 200 OK with 2 ndjson lines → clean EOF → reconnect
        if let Ok((mut conn, _)) = listener.accept().await {
            let mut buf = vec![0u8; 4096];
            let _ = conn.read(&mut buf).await;
            let body = "{\"msg\":\"hello\"}\n{\"msg\":\"world\"}\n";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = conn.write_all(resp.as_bytes()).await;
            // drop conn → EOF
        }
        // Round 2: 404 → bump_failures(0, 1) → exit
        if let Ok((mut conn, _)) = listener.accept().await {
            let mut buf = vec![0u8; 4096];
            let _ = conn.read(&mut buf).await;
            let _ = conn
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .await;
        }
    });

    let (tx, mut rx) = mpsc::channel(16);
    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        run(
            path,
            "/v1/logs/follow".to_owned(),
            tx,
            Liveness::new(60),
            default_conn(),
            fast_retry(1),
            passthrough(),
        ),
    )
    .await
    .expect("timed out");

    let line1 = rx.try_recv().expect("expected line1 in channel");
    let line2 = rx.try_recv().expect("expected line2 in channel");
    assert!(line1.contains("hello"), "unexpected line1: {line1}");
    assert!(line2.contains("world"), "unexpected line2: {line2}");
}

#[tokio::test]
async fn idle_timeout_causes_exit() {
    let path = tmp_socket();
    // Server sends 200 OK with chunked headers but never sends body data.
    start_unix_mock(
        &path,
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
        5_000,
    )
    .await;

    let conn = SourceConnConfig {
        idle_timeout_ms: 80,
        ..default_conn()
    };

    let (tx, _rx) = mpsc::channel(8);
    let err = tokio::time::timeout(
        Duration::from_secs(5),
        run(
            path,
            "/v1/logs/follow".to_owned(),
            tx,
            Liveness::new(60),
            conn,
            fast_retry(1),
            passthrough(),
        ),
    )
    .await
    .expect("timed out")
    .unwrap_err();

    assert!(
        err.to_string().contains("consecutive reconnect failures"),
        "unexpected error: {err}"
    );
}
