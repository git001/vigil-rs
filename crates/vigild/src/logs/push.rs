// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tracing::debug;
use vigil_types::api::LogEntry;

use super::store::LogStore;

/// Wraps a buffered writer over either a Unix socket or a TCP stream so the
/// inner push loop can be generic without dynamic dispatch.
pub(super) enum PushStream {
    Unix(tokio::io::BufWriter<tokio::net::UnixStream>),
    Tcp(tokio::io::BufWriter<tokio::net::TcpStream>),
}

impl PushStream {
    pub(super) async fn write_line(&mut self, line: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Unix(w) => {
                w.write_all(line).await?;
                w.flush().await
            }
            Self::Tcp(w) => {
                w.write_all(line).await?;
                w.flush().await
            }
        }
    }
}

/// Inner write loop. Returns `true` when the broadcast channel is closed
/// (daemon shutting down) and `false` when the connection is lost.
pub(super) async fn push_loop(
    service: &str,
    stream: &mut PushStream,
    rx: &mut broadcast::Receiver<LogEntry>,
) -> bool {
    loop {
        match rx.recv().await {
            Ok(entry) if entry.service == service => {
                let mut line = serde_json::to_string(&entry).unwrap_or_default();
                line.push('\n');
                if stream.write_line(line.as_bytes()).await.is_err() {
                    return false;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return true,
            _ => {}
        }
    }
}

/// Spawn a task that connects to `socket_path` (Unix domain socket) and
/// pushes ndjson log entries for `service`. Reconnects with exponential
/// backoff on failure. Abort the returned handle to stop the task.
pub fn spawn_push_unix(
    service: String,
    socket_path: String,
    store: Arc<LogStore>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff = Duration::from_millis(500);
        loop {
            match tokio::net::UnixStream::connect(&socket_path).await {
                Ok(s) => {
                    backoff = Duration::from_millis(500);
                    let mut ps = PushStream::Unix(tokio::io::BufWriter::new(s));
                    let mut rx = store.subscribe();
                    if push_loop(&service, &mut ps, &mut rx).await {
                        return; // broadcast closed — daemon shutting down
                    }
                    debug!(service = %service, path = %socket_path, "push socket lost, reconnecting");
                }
                Err(e) => {
                    debug!(service = %service, path = %socket_path, error = %e,
                           delay = ?backoff, "push connect failed");
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    })
}

/// Spawn a task that connects to `addr` (TCP `host:port`) and pushes ndjson
/// log entries for `service`. Reconnects with exponential backoff on failure.
/// Abort the returned handle to stop the task.
pub fn spawn_push_tcp(
    service: String,
    addr: String,
    store: Arc<LogStore>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff = Duration::from_millis(500);
        loop {
            match tokio::net::TcpStream::connect(&addr).await {
                Ok(s) => {
                    backoff = Duration::from_millis(500);
                    let mut ps = PushStream::Tcp(tokio::io::BufWriter::new(s));
                    let mut rx = store.subscribe();
                    if push_loop(&service, &mut ps, &mut rx).await {
                        return;
                    }
                    debug!(service = %service, addr = %addr, "push TCP lost, reconnecting");
                }
                Err(e) => {
                    debug!(service = %service, addr = %addr, error = %e,
                           delay = ?backoff, "push connect failed");
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    })
}
