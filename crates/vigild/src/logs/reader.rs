// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;

use chrono::Utc;
use tokio::io::BufReader;
use tracing::{debug, warn};
use vigil_types::api::{LogEntry, LogStream};
use vigil_types::lines::{EntryReader, LineReader, Multiline};
use vigil_types::plan::LogsMultiline;

use super::store::LogStore;

/// Read a service's stdout or stderr into `store` until the pipe closes.
///
/// The output may be anything: invalid UTF-8 becomes `�`, and a line
/// longer than the store's `max_line` is stored in several entries. With
/// `multiline` the lines of a stack trace become one entry (with `\n`
/// between them). The pipe is read to its end in any case: a reader that
/// stopped early would close it, and the service would die of SIGPIPE at
/// its next write.
pub fn spawn_reader(
    service: String,
    stream_type: LogStream,
    reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    store: Arc<LogStore>,
    forward: bool,
    multiline: Option<&LogsMultiline>,
) {
    let max = store.max_line();
    // Validated when the layers were loaded; a failure here only drops the
    // joining, never the output.
    let multiline = multiline.and_then(|c| {
        Multiline::from_config(c, max)
            .inspect_err(|e| warn!(service = %service, error = %e, "logs-multiline ignored"))
            .ok()
    });
    tokio::spawn(async move {
        let lines = LineReader::new(BufReader::new(reader), max);
        let mut entries = EntryReader::new(lines, multiline);
        loop {
            let message = match entries.next_entry().await {
                Ok(Some(entry)) => entry.text,
                Ok(None) => break,
                Err(e) => {
                    debug!(service = %service, error = %e, "reading service output failed");
                    break;
                }
            };
            if forward {
                // Mirror to vigild's stdout/stderr so `podman logs` /
                // `docker logs` capture the service output: stdout lines
                // to vigild's stdout, stderr lines to its stderr. Only the
                // first line of a multi-line entry gets the prefix, so the
                // rest looks as the service wrote it.
                match stream_type {
                    LogStream::Stdout => println!("[{service}] {message}"),
                    LogStream::Stderr => eprintln!("[{service}] {message}"),
                }
            }
            store
                .push(LogEntry {
                    timestamp: Utc::now(),
                    service: service.clone(),
                    stream: stream_type,
                    message,
                })
                .await;
        }
    });
}
