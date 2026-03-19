//! Per-pod log streaming task.

use std::collections::HashMap;
use std::sync::Arc;

use k8s_openapi::api::core::v1::Pod;
use kube::{api::LogParams, Api};
use serde::Serialize;
use tokio::io::AsyncBufReadExt;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{info, warn};

use crate::LineFilter;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// ndjson record produced in --kubernetes mode
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct LogEntry<'a> {
    timestamp: &'a str,
    namespace: &'a str,
    pod: &'a str,
    stream: &'static str,
    message: &'a str,
}

// ---------------------------------------------------------------------------
// Start a stream task if none is active for this pod.
// Returns true if a new task was spawned.
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn try_start(
    pod: &str,
    active: &mut HashMap<String, tokio::task::JoinHandle<()>>,
    semaphore: &Option<Arc<tokio::sync::Semaphore>>,
    pods_api: &Api<Pod>,
    namespace: &Arc<String>,
    container: &Option<Arc<String>>,
    tail_lines: i64,
    since_seconds: i64,
    filter: &Arc<LineFilter>,
    tx: &mpsc::Sender<String>,
) -> bool {
    if active.contains_key(pod) { return false; }

    let permit = match semaphore {
        Some(sem) => match sem.clone().try_acquire_owned() {
            Ok(p)  => Some(p),
            Err(_) => {
                warn!(pod = %pod, "at max-log-requests capacity — deferring to next reconcile");
                return false;
            }
        },
        None => None,
    };

    info!(pod = %pod, "starting log stream");
    let fut = stream_pod(
        pods_api.clone(),
        pod.to_owned(),
        Arc::clone(namespace),
        container.clone(),
        tail_lines,
        since_seconds,
        Arc::clone(filter),
        tx.clone(),
    );
    let handle = tokio::spawn(async move {
        let _permit = permit; // released when task ends, freeing semaphore slot
        fut.await;
    });
    active.insert(pod.to_owned(), handle);
    true
}

// ---------------------------------------------------------------------------
// Per-pod streaming task
// ---------------------------------------------------------------------------

async fn stream_pod(
    pods: Api<Pod>,
    pod: String,
    namespace: Arc<String>,
    container: Option<Arc<String>>,
    tail_lines: i64,
    since_seconds: i64,
    filter: Arc<LineFilter>,
    tx: mpsc::Sender<String>,
) {
    let params = LogParams {
        follow: true,
        timestamps: true,
        container: container.as_deref().map(|s| s.to_owned()),
        tail_lines:    if tail_lines   > 0 { Some(tail_lines)   } else { None },
        since_seconds: if tail_lines == 0 && since_seconds > 0 { Some(since_seconds) } else { None },
        ..Default::default()
    };

    let byte_stream = match pods.log_stream(&pod, &params).await {
        Ok(s)  => s,
        Err(e) => {
            warn!(pod = %pod, error = %e, "failed to open log stream");
            return;
        }
    };

    // kube returns futures_io::AsyncBufRead; .compat() bridges it to tokio::AsyncRead
    let mut lines = tokio::io::BufReader::new(byte_stream.compat()).lines();

    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                // K8s timestamp-prefixed lines: "2026-03-18T11:23:45.123Z <message>"
                let (ts, msg) = match line.find(' ') {
                    Some(i) => (&line[..i], &line[i + 1..]),
                    None    => (line.as_str(), ""),
                };

                if !filter.allow(msg) { continue; }

                let entry = LogEntry {
                    timestamp: ts,
                    namespace: &namespace,
                    pod: &pod,
                    stream: "stdout",
                    message: msg,
                };
                if let Ok(mut json) = serde_json::to_string(&entry) {
                    json.push('\n');
                    if tx.try_send(json).is_err() {
                        warn!(pod = %pod, "send buffer full — dropping log line");
                    }
                }
            }
            Ok(None) => {
                info!(pod = %pod, "log stream EOF");
                break;
            }
            Err(e) => {
                warn!(pod = %pod, error = %e, "log stream error");
                break;
            }
        }
    }
}
