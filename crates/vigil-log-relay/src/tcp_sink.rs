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
                    // Receive next line (with optional idle timeout)
                    let line = if cfg.idle_timeout_ms > 0 {
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

fn apply_keepalive(stream: &TcpStream, cfg: &SinkConfig) {
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
