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
//! with exponential backoff (500 ms → 30 s) on write or connect failure.

use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{info, warn};

pub async fn run(addr: String, mut rx: mpsc::Receiver<String>) {
    let mut backoff = Duration::from_millis(500);

    loop {
        match TcpStream::connect(&addr).await {
            Ok(mut stream) => {
                info!(addr = %addr, "tcp sink connected");
                backoff = Duration::from_millis(500);

                loop {
                    match rx.recv().await {
                        Some(line) => {
                            if let Err(e) = stream.write_all(line.as_bytes()).await {
                                warn!(addr = %addr, error = %e, "write error — reconnecting");
                                break;
                            }
                        }
                        None => {
                            info!("channel closed — tcp sink exiting");
                            return;
                        }
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
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}
