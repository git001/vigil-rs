// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;

use chrono::Utc;
use tokio::io::{AsyncBufReadExt, BufReader};
use vigil_types::api::{LogEntry, LogStream};

use super::store::LogStore;

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
