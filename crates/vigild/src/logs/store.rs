// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock, broadcast};
use vigil_types::api::LogEntry;

/// Default per-service ring-buffer capacity (number of log lines kept in memory).
pub const DEFAULT_BUFFER_CAPACITY: usize = 1_000;

/// Smallest allowed `--log-max-line-bytes`.
pub const MIN_MAX_LINE: usize = 1024;
/// Largest allowed `--log-max-line-bytes`: a full buffer of such entries
/// costs `--log-buffer` times this per service.
pub const MAX_MAX_LINE: usize = 1024 * 1024;

/// The log settings of vigild (`--log-buffer`, `--log-max-line-bytes`).
#[derive(Debug, Clone, Copy)]
pub struct LogOptions {
    /// Entries kept per service.
    pub buffer: usize,
    /// Longest entry in bytes; a longer line is split.
    pub max_line: usize,
}

// ---------------------------------------------------------------------------
// Per-service ring buffer
// ---------------------------------------------------------------------------

pub(super) struct Buffer {
    inner: Mutex<VecDeque<LogEntry>>,
    capacity: usize,
}

impl Buffer {
    pub(super) fn new(capacity: usize) -> Self {
        Buffer {
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    pub(super) async fn push(&self, entry: LogEntry) {
        let mut g = self.inner.lock().await;
        if g.len() >= self.capacity {
            g.pop_front(); // drop oldest
        }
        g.push_back(entry);
    }

    pub(super) async fn snapshot(&self) -> Vec<LogEntry> {
        self.inner.lock().await.iter().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// LogStore — shared across the whole daemon
// ---------------------------------------------------------------------------

pub struct LogStore {
    pub(super) buffers: RwLock<HashMap<String, Arc<Buffer>>>,
    pub(super) broadcast: broadcast::Sender<LogEntry>,
    pub(super) buffer_capacity: usize,
    max_line: usize,
}

impl LogStore {
    /// Create a new `LogStore`.
    ///
    /// * `buffer_capacity`   — number of lines kept per service (ring buffer).
    /// * `broadcast_capacity` — broadcast channel depth; slow followers are
    ///   notified via a `Lagged` event and skip ahead rather than blocking.
    pub fn new(buffer_capacity: usize, broadcast_capacity: usize) -> Arc<Self> {
        Self::with_max_line(
            buffer_capacity,
            broadcast_capacity,
            vigil_types::lines::DEFAULT_MAX_LINE,
        )
    }

    /// As [`LogStore::new`], with entries of at most `max_line` bytes.
    pub fn with_max_line(
        buffer_capacity: usize,
        broadcast_capacity: usize,
        max_line: usize,
    ) -> Arc<Self> {
        let (broadcast, _) = broadcast::channel(broadcast_capacity);
        Arc::new(LogStore {
            buffers: RwLock::new(HashMap::new()),
            broadcast,
            buffer_capacity,
            max_line,
        })
    }

    /// Longest entry in bytes (`--log-max-line-bytes`).
    pub fn max_line(&self) -> usize {
        self.max_line
    }

    pub(super) async fn buffer_for(&self, service: &str) -> Arc<Buffer> {
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
        self.buffer_for(&entry.service)
            .await
            .push(entry.clone())
            .await;
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
