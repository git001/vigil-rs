//! TCP sink — single task that owns the TCP connection and writes ndjson lines.
//!
//! **Output format: newline-delimited JSON (ndjson) only.**
//! Every line written to the sink is a complete, self-contained JSON object
//! followed by `\n`. The sink makes no attempt to parse or validate the JSON;
//! it forwards bytes as-is. Receivers must accept raw ndjson (e.g. Filebeat
//! `tcp` input, Fluent Bit `tcp` input, Logstash `tcp` input with json codec).
//!
//! All source tasks send their serialised lines through a shared mpsc channel.
//! This task drains the channel and writes to the TCP listener, reconnecting
//! with exponential backoff on write or connect failure.

use std::time::Duration;

use socket2::{SockRef, TcpKeepalive};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

pub struct SinkConfig {
    /// TCP connect timeout (0 = no timeout).
    pub connect_timeout_ms: u64,
    /// Per-write timeout; reconnects if write stalls (0 = disabled).
    pub write_timeout_ms: u64,
    /// Idle timeout; reconnects if channel stays empty this long (0 = disabled).
    pub idle_timeout_ms: u64,
    /// TCP keepalive interval in seconds (0 = disabled).
    pub keepalive_interval_secs: u64,
    /// TCP keepalive probe timeout in seconds (0 = OS default).
    pub keepalive_timeout_secs: u64,
    /// Initial reconnect delay in ms.
    pub reconnect_delay_ms: u64,
    /// Reconnect backoff ceiling in ms.
    pub reconnect_max_ms: u64,
}

// ---------------------------------------------------------------------------
// Run loop
// ---------------------------------------------------------------------------

pub async fn run(addr: String, mut rx: mpsc::Receiver<String>, cfg: SinkConfig) {
    let mut backoff = Duration::from_millis(cfg.reconnect_delay_ms);
    // Line that failed to write — retried first on the next connection
    // so no log entries are lost on Broken pipe / write timeout.
    let mut pending: Option<String> = None;

    loop {
        let connect = TcpStream::connect(&addr);
        let stream_result = if cfg.connect_timeout_ms > 0 {
            match timeout(Duration::from_millis(cfg.connect_timeout_ms), connect).await {
                Ok(r) => r,
                Err(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "connect timeout",
                )),
            }
        } else {
            connect.await
        };

        match stream_result {
            Ok(mut stream) => {
                apply_keepalive(&stream, &cfg);
                info!(addr = %addr, "tcp sink connected");
                backoff = Duration::from_millis(cfg.reconnect_delay_ms);

                loop {
                    // Retry a previously failed line first; otherwise receive next.
                    let line = if let Some(p) = pending.take() {
                        p
                    } else if cfg.idle_timeout_ms > 0 {
                        match timeout(Duration::from_millis(cfg.idle_timeout_ms), rx.recv()).await {
                            Ok(Some(l)) => l,
                            Ok(None) => {
                                info!("channel closed — tcp sink exiting");
                                return;
                            }
                            Err(_) => {
                                info!(addr = %addr, "idle timeout — reconnecting");
                                break;
                            }
                        }
                    } else {
                        match rx.recv().await {
                            Some(l) => l,
                            None => {
                                info!("channel closed — tcp sink exiting");
                                return;
                            }
                        }
                    };

                    // Write with optional per-write timeout
                    let write_result = if cfg.write_timeout_ms > 0 {
                        match timeout(
                            Duration::from_millis(cfg.write_timeout_ms),
                            stream.write_all(line.as_bytes()),
                        )
                        .await
                        {
                            Ok(r) => r,
                            Err(_) => Err(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "write timeout",
                            )),
                        }
                    } else {
                        stream.write_all(line.as_bytes()).await
                    };

                    if let Err(e) = write_result {
                        warn!(addr = %addr, error = %e, "write error — reconnecting");
                        pending = Some(line); // preserve for retry after reconnect
                        break;
                    }
                }
            }
            Err(e) => {
                warn!(
                    addr       = %addr,
                    error      = %e,
                    backoff_ms = backoff.as_millis(),
                    "tcp sink cannot connect",
                );
                sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_millis(cfg.reconnect_max_ms));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TCP keepalive helper
// ---------------------------------------------------------------------------

pub fn apply_keepalive(stream: &TcpStream, cfg: &SinkConfig) {
    if cfg.keepalive_interval_secs == 0 {
        return;
    }
    let mut ka = TcpKeepalive::new()
        .with_time(Duration::from_secs(if cfg.keepalive_timeout_secs > 0 {
            cfg.keepalive_timeout_secs
        } else {
            cfg.keepalive_interval_secs * 3 // sensible default: 3× interval
        }))
        .with_interval(Duration::from_secs(cfg.keepalive_interval_secs));

    #[cfg(not(target_os = "windows"))]
    {
        ka = ka.with_retries(3);
    }

    if let Err(e) = SockRef::from(stream).set_tcp_keepalive(&ka) {
        warn!(error = %e, "failed to set TCP keepalive");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    fn default_cfg() -> SinkConfig {
        SinkConfig {
            connect_timeout_ms: 1000,
            write_timeout_ms: 0,
            idle_timeout_ms: 0,
            keepalive_interval_secs: 0,
            keepalive_timeout_secs: 0,
            reconnect_delay_ms: 10,
            reconnect_max_ms: 100,
        }
    }

    #[tokio::test]
    async fn channel_close_after_connect_exits() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (tx, rx) = mpsc::channel::<String>(8);

        // Accept one connection in the background
        tokio::spawn(async move {
            let _ = listener.accept().await;
            // keep connection alive briefly, then drop it
        });

        // Close the sender immediately — run() should exit after connecting
        drop(tx);

        // run() will connect, then rx.recv() returns None → exits
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            run(addr, rx, default_cfg()),
        )
        .await
        .expect("run() did not exit after channel was closed");
    }

    #[tokio::test]
    async fn writes_lines_to_sink() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (tx, rx) = mpsc::channel::<String>(8);

        // Accept and collect received data
        let collect = tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            let mut buf = String::new();
            // Read up to 3 lines then return
            let mut line_buf = [0u8; 4096];
            loop {
                let n = conn.read(&mut line_buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.push_str(&String::from_utf8_lossy(&line_buf[..n]));
                if buf.matches('\n').count() >= 3 {
                    break;
                }
            }
            buf
        });

        // Send 3 lines
        tx.send("line-one\n".to_string()).await.unwrap();
        tx.send("line-two\n".to_string()).await.unwrap();
        tx.send("line-three\n".to_string()).await.unwrap();

        // Start run() — it will connect and write the lines
        let run_handle = tokio::spawn(run(addr, rx, default_cfg()));

        let received = tokio::time::timeout(std::time::Duration::from_secs(5), collect)
            .await
            .expect("collect timed out")
            .unwrap();

        assert!(
            received.contains("line-one"),
            "missing line-one in: {received}"
        );
        assert!(
            received.contains("line-two"),
            "missing line-two in: {received}"
        );
        assert!(
            received.contains("line-three"),
            "missing line-three in: {received}"
        );

        run_handle.abort();
    }

    #[tokio::test]
    async fn apply_keepalive_zero_interval_is_noop() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let stream = TcpStream::connect(addr).await.unwrap();
        // keepalive_interval_secs = 0 → early return, no panic
        apply_keepalive(&stream, &default_cfg());
    }

    #[tokio::test]
    async fn apply_keepalive_nonzero_does_not_panic() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let stream = TcpStream::connect(addr).await.unwrap();
        let cfg = SinkConfig {
            keepalive_interval_secs: 10,
            keepalive_timeout_secs: 30,
            ..default_cfg()
        };
        apply_keepalive(&stream, &cfg);
    }
}
