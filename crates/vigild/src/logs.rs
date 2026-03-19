// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock, broadcast};
use tracing::debug;
use vigil_types::api::{LogEntry, LogStream};

/// Default per-service ring-buffer capacity (number of log lines kept in memory).
pub const DEFAULT_BUFFER_CAPACITY: usize = 1_000;

// ---------------------------------------------------------------------------
// Per-service ring buffer
// ---------------------------------------------------------------------------

struct Buffer {
    inner:    Mutex<VecDeque<LogEntry>>,
    capacity: usize,
}

impl Buffer {
    fn new(capacity: usize) -> Self {
        Buffer {
            inner:    Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    async fn push(&self, entry: LogEntry) {
        let mut g = self.inner.lock().await;
        if g.len() >= self.capacity {
            g.pop_front(); // drop oldest
        }
        g.push_back(entry);
    }

    async fn snapshot(&self) -> Vec<LogEntry> {
        self.inner.lock().await.iter().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// LogStore — shared across the whole daemon
// ---------------------------------------------------------------------------

pub struct LogStore {
    buffers:         RwLock<HashMap<String, Arc<Buffer>>>,
    broadcast:       broadcast::Sender<LogEntry>,
    buffer_capacity: usize,
}

impl LogStore {
    /// Create a new `LogStore`.
    ///
    /// * `buffer_capacity`   — number of lines kept per service (ring buffer).
    /// * `broadcast_capacity` — broadcast channel depth; slow followers are
    ///   notified via a `Lagged` event and skip ahead rather than blocking.
    pub fn new(buffer_capacity: usize, broadcast_capacity: usize) -> Arc<Self> {
        let (broadcast, _) = broadcast::channel(broadcast_capacity);
        Arc::new(LogStore {
            buffers: RwLock::new(HashMap::new()),
            broadcast,
            buffer_capacity,
        })
    }

    async fn buffer_for(&self, service: &str) -> Arc<Buffer> {
        {
            let r = self.buffers.read().await;
            if let Some(b) = r.get(service) {
                return Arc::clone(b);
            }
        }
        let mut w = self.buffers.write().await;
        let b = Arc::new(Buffer::new(self.buffer_capacity));
        w.insert(service.to_string(), Arc::clone(&b));
        b
    }

    pub async fn push(&self, entry: LogEntry) {
        self.buffer_for(&entry.service).await.push(entry.clone()).await;
        // Ignore send errors: no active followers is not an error.
        let _ = self.broadcast.send(entry);
    }

    /// Return up to `n` most recent lines across the requested services.
    pub async fn tail(&self, services: &[String], n: usize) -> Vec<LogEntry> {
        let r = self.buffers.read().await;
        let mut all: Vec<LogEntry> = Vec::new();
        for (name, buf) in r.iter() {
            if services.is_empty() || services.contains(name) {
                all.extend(buf.snapshot().await);
            }
        }
        all.sort_by_key(|e| e.timestamp);
        let len = all.len();
        if len > n { all.split_off(len - n) } else { all }
    }

    /// Subscribe to the live log stream.
    /// The receiver will get every entry pushed after this call.
    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.broadcast.subscribe()
    }
}

// ---------------------------------------------------------------------------
// Log-push tasks — vigild connects to an external socket and pushes ndjson
// ---------------------------------------------------------------------------

/// Wraps a buffered writer over either a Unix socket or a TCP stream so the
/// inner push loop can be generic without dynamic dispatch.
enum PushStream {
    Unix(tokio::io::BufWriter<tokio::net::UnixStream>),
    Tcp(tokio::io::BufWriter<tokio::net::TcpStream>),
}

impl PushStream {
    async fn write_line(&mut self, line: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Unix(w) => { w.write_all(line).await?; w.flush().await }
            Self::Tcp(w)  => { w.write_all(line).await?; w.flush().await }
        }
    }
}

/// Inner write loop. Returns `true` when the broadcast channel is closed
/// (daemon shutting down) and `false` when the connection is lost.
async fn push_loop(
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

// ---------------------------------------------------------------------------
// Async log-reader task — reads lines from a pipe and feeds LogStore
// ---------------------------------------------------------------------------

pub fn spawn_reader(
    service: String,
    stream_type: LogStream,
    reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    store: Arc<LogStore>,
    forward: bool,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if forward {
                // Mirror to vigild's stdout/stderr so `podman logs` /
                // `docker logs` capture the service output.
                // stdout lines → vigild stdout, stderr lines → vigild stderr.
                match stream_type {
                    LogStream::Stdout => println!("[{service}] {line}"),
                    LogStream::Stderr => eprintln!("[{service}] {line}"),
                }
            }
            store
                .push(LogEntry {
                    timestamp: Utc::now(),
                    service: service.clone(),
                    stream: stream_type,
                    message: line,
                })
                .await;
        }
    });
}
