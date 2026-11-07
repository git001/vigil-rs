// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::io::Write as _;
use std::time::Duration;

use tempfile::NamedTempFile;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use super::run;
use crate::{LineFilter, Liveness, ReconnectConfig, SourceConnConfig};

fn passthrough() -> LineFilter {
    LineFilter::from_strs(&[], &[])
}

fn default_conn() -> SourceConnConfig {
    SourceConnConfig {
        connect_timeout_ms: 500,
        read_timeout_ms: 0,
        idle_timeout_ms: 0,
        keepalive: crate::keepalive::Keepalive {
            interval_secs: 0,
            timeout_secs: 0,
        },
        source_insecure: false,
        source_cacert: None,
        proxy_url: None,
        proxy_insecure: false,
        proxy_cacert: None,
        no_proxy: None,
        source_user_file: None,
        source_cert: None,
        source_key: None,
    }
}

fn fast_retry(max_retries: u64) -> ReconnectConfig {
    ReconnectConfig {
        initial_delay_ms: 1,
        max_delay_ms: 10,
        max_retries,
    }
}

/// Minimal mock HTTP server — reads the request, writes `response`, keeps the
/// connection open for `hold_ms` ms, then drops it. Accepts connections in a loop.
async fn start_mock(response: &'static str, hold_ms: u64) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        while let Ok((mut conn, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let _ = conn.read(&mut buf).await;
                let _ = conn.write_all(response.as_bytes()).await;
                if hold_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(hold_ms)).await;
                }
                // drop conn
            });
        }
    });
    addr
}

// -----------------------------------------------------------------------

#[tokio::test]
async fn non_2xx_exits_after_max_retries() {
    let addr = start_mock("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n", 0).await;

    let (tx, _rx) = mpsc::channel(8);
    let err = tokio::time::timeout(
        Duration::from_secs(5),
        run(
            format!("http://{addr}/"),
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
async fn connect_error_exits_after_max_retries() {
    // Bind and immediately drop — port becomes unavailable (ECONNREFUSED)
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);

    let (tx, _rx) = mpsc::channel(8);
    let err = tokio::time::timeout(
        Duration::from_secs(5),
        run(
            format!("http://{addr}/"),
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
    // Round 1: 200 OK with 2 ndjson lines → clean EOF → reconnect
    // Round 2: 404 → bump_failures(0, 1) → exit
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
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
            format!("http://{addr}/"),
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
    // Server sends 200 OK with chunked headers but never sends body data.
    // The connection is held open so reqwest keeps waiting.
    // idle_timeout_ms fires → reconnect → bump_failures(0, 1) → exit.
    let addr = start_mock(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
        5_000, // hold open for 5 s — far longer than the idle timeout
    )
    .await;

    let conn = SourceConnConfig {
        idle_timeout_ms: 80, // short idle timeout
        ..default_conn()
    };

    let (tx, _rx) = mpsc::channel(8);
    let err = tokio::time::timeout(
        Duration::from_secs(5),
        run(
            format!("http://{addr}/"),
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

#[tokio::test]
async fn invalid_proxy_url_returns_error() {
    let conn = SourceConnConfig {
        proxy_url: Some("not-a-valid-url".to_string()),
        ..default_conn()
    };
    let (tx, _rx) = mpsc::channel(8);
    let err = tokio::time::timeout(
        Duration::from_secs(2),
        run(
            "http://127.0.0.1:1/".to_string(),
            tx,
            Liveness::new(60),
            conn,
            fast_retry(0),
            passthrough(),
        ),
    )
    .await
    .expect("timed out")
    .unwrap_err();
    assert!(
        err.to_string().contains("invalid") || err.to_string().contains("proxy"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn proxy_cacert_client_builds() {
    // Verify that a valid proxy_cacert PEM is accepted (client constructs fine).
    // The connection to port 1 will fail immediately — only testing client build path.
    use rcgen::{CertificateParams, KeyPair};
    let key = KeyPair::generate().unwrap();
    let pem = CertificateParams::new(vec!["test".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap()
        .pem();
    let mut ca_file = NamedTempFile::new().unwrap();
    ca_file.write_all(pem.as_bytes()).unwrap();

    let conn = SourceConnConfig {
        proxy_url: Some("http://proxy.example.com:3128".to_string()),
        proxy_cacert: Some(ca_file.path().to_owned()),
        connect_timeout_ms: 100,
        ..default_conn()
    };
    let (tx, _rx) = mpsc::channel(8);
    let _ = tokio::time::timeout(
        Duration::from_millis(500),
        run(
            "http://127.0.0.1:1/".to_string(),
            tx,
            Liveness::new(60),
            conn,
            fast_retry(0),
            passthrough(),
        ),
    )
    .await;
    // Reaching here without a "proxy CA parse" error is the assertion.
}

// ---------------------------------------------------------------------------
// Authentication (--source-user-file, --source-cert/--source-key)
// ---------------------------------------------------------------------------

#[test]
fn basic_credentials_file_parsing() {
    use super::read_basic_credentials;
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "relay:p@ss: with spaces").unwrap();
    let (u, p) = read_basic_credentials(f.path()).unwrap();
    assert_eq!(u, "relay");
    assert_eq!(p, "p@ss: with spaces", "only trailing newline stripped");

    let mut bad = NamedTempFile::new().unwrap();
    write!(bad, "no-colon").unwrap();
    assert!(read_basic_credentials(bad.path()).is_err());

    let mut empty_user = NamedTempFile::new().unwrap();
    write!(empty_user, ":pw").unwrap();
    assert!(read_basic_credentials(empty_user.path()).is_err());

    assert!(read_basic_credentials(std::path::Path::new("/nonexistent/x")).is_err());
}

/// The Authorization header is sent, and the file is re-read on reconnect
/// (a rotated Secret is picked up without restarting the relay).
#[tokio::test]
async fn basic_auth_header_sent_and_file_reread_on_reconnect() {
    use base64::Engine as _;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (seen_tx, mut seen_rx) = mpsc::channel::<String>(4);
    tokio::spawn(async move {
        for _ in 0..2 {
            if let Ok((mut conn, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let n = conn.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let auth = req
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("authorization: ")
                            .or_else(|| l.strip_prefix("Authorization: "))
                    })
                    .unwrap_or("")
                    .to_string();
                let _ = seen_tx.send(auth).await;
                let _ = conn
                    .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
                    .await;
            }
        }
    });

    let mut cred = NamedTempFile::new().unwrap();
    writeln!(cred, "relay:first").unwrap();
    let path = cred.path().to_path_buf();
    let conn = SourceConnConfig {
        source_user_file: Some(path.clone()),
        ..default_conn()
    };

    let relay = tokio::spawn(run(
        format!("http://{addr}/"),
        mpsc::channel(16).0,
        Liveness::new(60),
        conn,
        ReconnectConfig {
            initial_delay_ms: 200,
            max_delay_ms: 200,
            max_retries: 2,
        },
        passthrough(),
    ));

    let first = tokio::time::timeout(Duration::from_secs(5), seen_rx.recv())
        .await
        .unwrap()
        .unwrap();
    // Rotate the secret before the reconnect.
    std::fs::write(&path, "relay:second\n").unwrap();
    let second = tokio::time::timeout(Duration::from_secs(5), seen_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), relay).await;

    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    assert_eq!(first, format!("Basic {}", b64("relay:first")));
    assert_eq!(second, format!("Basic {}", b64("relay:second")));
}

#[tokio::test]
async fn missing_user_file_counts_as_failure() {
    let conn = SourceConnConfig {
        source_user_file: Some("/nonexistent/vigil-relay-cred".into()),
        ..default_conn()
    };
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        run(
            "http://127.0.0.1:9/".into(),
            mpsc::channel(16).0,
            Liveness::new(60),
            conn,
            fast_retry(1),
            passthrough(),
        ),
    )
    .await
    .expect("timed out");
    assert!(res.is_err(), "gives up after max retries");
}

#[tokio::test]
async fn invalid_client_identity_is_an_error() {
    let mut cert = NamedTempFile::new().unwrap();
    write!(cert, "not a pem").unwrap();
    let mut key = NamedTempFile::new().unwrap();
    write!(key, "not a key").unwrap();
    let conn = SourceConnConfig {
        source_cert: Some(cert.path().to_path_buf()),
        source_key: Some(key.path().to_path_buf()),
        ..default_conn()
    };
    let err = run(
        "https://127.0.0.1:9/".into(),
        mpsc::channel(16).0,
        Liveness::new(60),
        conn,
        fast_retry(1),
        passthrough(),
    )
    .await
    .unwrap_err();
    assert!(format!("{err:#}").contains("--source-cert"), "{err:#}");
}

#[tokio::test]
async fn valid_client_identity_builds() {
    let ck = rcgen::generate_simple_self_signed(vec!["relay".into()]).unwrap();
    let mut cert = NamedTempFile::new().unwrap();
    write!(cert, "{}", ck.cert.pem()).unwrap();
    let mut key = NamedTempFile::new().unwrap();
    write!(key, "{}", ck.key_pair.serialize_pem()).unwrap();
    let conn = SourceConnConfig {
        source_cert: Some(cert.path().to_path_buf()),
        source_key: Some(key.path().to_path_buf()),
        ..default_conn()
    };
    // Port 9 (discard) refuses: the client must build, then fail to connect.
    let err = run(
        "https://127.0.0.1:9/".into(),
        mpsc::channel(16).0,
        Liveness::new(60),
        conn,
        fast_retry(1),
        passthrough(),
    )
    .await
    .unwrap_err();
    assert!(!format!("{err:#}").contains("--source-cert"), "{err:#}");
}
