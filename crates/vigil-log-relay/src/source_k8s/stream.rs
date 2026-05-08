//! Per-pod log streaming task.

use std::collections::HashMap;
use std::sync::Arc;

use kube::Client;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    container: Option<&'a str>,
    stream: &'static str,
    message: &'a str,
}

// ---------------------------------------------------------------------------
// Start a stream task if none is active for this (pod, container_key) pair.
// container_key="" means no &container= param (K8s picks the default).
// Returns true if a new task was spawned.
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn try_start(
    pod: &str,
    container_key: &str,
    active: &mut HashMap<(String, String), tokio::task::JoinHandle<()>>,
    semaphore: &Option<Arc<tokio::sync::Semaphore>>,
    kube_client: &Client,
    namespace: &Arc<String>,
    tail_lines: i64,
    since_seconds: i64,
    use_stream_param: bool,
    filter: &Arc<LineFilter>,
    tx: &mpsc::Sender<String>,
) -> bool {
    let key = (pod.to_owned(), container_key.to_owned());
    if active.contains_key(&key) {
        return false;
    }

    let permit = match semaphore {
        Some(sem) => match sem.clone().try_acquire_owned() {
            Ok(p) => Some(p),
            Err(_) => {
                warn!(pod = %pod, container = %container_key, "at max-log-requests capacity — deferring to next reconcile");
                return false;
            }
        },
        None => None,
    };

    let container_name: Option<Arc<String>> = if container_key.is_empty() {
        None
    } else {
        Some(Arc::new(container_key.to_owned()))
    };

    if let Some(ref cname) = container_name {
        info!(pod = %pod, container = %cname.as_str(), "starting log stream");
    } else {
        info!(pod = %pod, "starting log stream");
    }

    let handle = if use_stream_param {
        // K8s ≥ 1.32: request stdout and stderr as separate streams.
        let url_out = log_url(
            namespace,
            pod,
            container_key,
            tail_lines,
            since_seconds,
            Some("Stdout"),
        );
        let url_err = log_url(
            namespace,
            pod,
            container_key,
            tail_lines,
            since_seconds,
            Some("Stderr"),
        );
        let client = kube_client.clone();
        let pod_str = pod.to_owned();
        let ns = Arc::clone(namespace);
        let cname_out = container_name.clone();
        let cname_err = container_name.clone();
        let filter_out = Arc::clone(filter);
        let filter_err = Arc::clone(filter);
        let tx_out = tx.clone();
        let tx_err = tx.clone();

        tokio::spawn(async move {
            let _permit = permit;
            let mut h_out = tokio::spawn(stream_pod(
                client.clone(),
                url_out,
                pod_str.clone(),
                Arc::clone(&ns),
                cname_out,
                filter_out,
                tx_out,
                "stdout",
            ));
            let mut h_err = tokio::spawn(stream_pod(
                client, url_err, pod_str, ns, cname_err, filter_err, tx_err, "stderr",
            ));
            // When either stream closes (K8s API server timeout), abort the sibling.
            tokio::select! {
                _ = &mut h_out => { h_err.abort(); }
                _ = &mut h_err => { h_out.abort(); }
            }
        })
    } else {
        // K8s < 1.32: single combined stream, labeled "output".
        let url = log_url(
            namespace,
            pod,
            container_key,
            tail_lines,
            since_seconds,
            None,
        );
        let client = kube_client.clone();
        let pod_str = pod.to_owned();
        let ns = Arc::clone(namespace);
        let filter = Arc::clone(filter);
        let tx = tx.clone();

        tokio::spawn(async move {
            let _permit = permit;
            stream_pod(
                client,
                url,
                pod_str,
                ns,
                container_name,
                filter,
                tx,
                "output",
            )
            .await;
        })
    };

    active.insert(key, handle);
    true
}

// ---------------------------------------------------------------------------
// URL builder
// ---------------------------------------------------------------------------

fn log_url(
    namespace: &str,
    pod: &str,
    container_key: &str,
    tail_lines: i64,
    since_seconds: i64,
    stream: Option<&str>,
) -> String {
    let mut url =
        format!("/api/v1/namespaces/{namespace}/pods/{pod}/log?follow=true&timestamps=true");
    if !container_key.is_empty() {
        url.push_str(&format!("&container={container_key}"));
    }
    if tail_lines > 0 {
        url.push_str(&format!("&tailLines={tail_lines}"));
    } else if since_seconds > 0 {
        url.push_str(&format!("&sinceSeconds={since_seconds}"));
    }
    if let Some(s) = stream {
        url.push_str(&format!("&stream={s}"));
    }
    url
}

// ---------------------------------------------------------------------------
// Per-pod streaming task
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn stream_pod(
    client: Client,
    url: String,
    pod: String,
    namespace: Arc<String>,
    container: Option<Arc<String>>,
    filter: Arc<LineFilter>,
    tx: mpsc::Sender<String>,
    stream_name: &'static str,
) {
    let container_str = container.as_deref().map(|s| s.as_str());

    let req = match http::Request::get(&url).body(vec![]) {
        Ok(r) => r,
        Err(e) => {
            warn!(pod = %pod, stream = stream_name, error = %e, "failed to build log request");
            return;
        }
    };

    let byte_stream = match client.request_stream(req).await {
        Ok(s) => s,
        Err(e) => {
            let hint = if e.to_string().contains("may not be specified") {
                " — use --no-stream-param to disable stdout/stderr separation"
            } else {
                ""
            };
            warn!(pod = %pod, stream = stream_name, error = %e, hint, "failed to open log stream");
            return;
        }
    };

    let mut lines = byte_stream.compat().lines();

    // Deduplicate: K8s can deliver the same line twice near stream start when
    // tailLines + follow=true are combined (known K8s quirk). Skip any line
    // whose timestamp is not strictly greater than the last forwarded one.
    let mut last_ts = String::new();

    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                // K8s timestamp-prefixed lines: "2026-03-18T11:23:45.123Z <message>"
                let (ts, msg) = match line.find(' ') {
                    Some(i) => (&line[..i], &line[i + 1..]),
                    None => (line.as_str(), ""),
                };

                if ts <= last_ts.as_str() {
                    continue; // duplicate or out-of-order — skip
                }

                if !filter.allow(msg) {
                    continue;
                }

                let entry = LogEntry {
                    timestamp: ts,
                    namespace: &namespace,
                    pod: &pod,
                    container: container_str,
                    stream: stream_name,
                    message: msg,
                };
                if let Ok(mut json) = serde_json::to_string(&entry) {
                    json.push('\n');
                    if tx.try_send(json).is_err() {
                        warn!(pod = %pod, stream = stream_name, "send buffer full — dropping log line");
                    }
                }
                last_ts = ts.to_owned();
            }
            Ok(None) => {
                info!(pod = %pod, stream = stream_name, "log stream EOF");
                break;
            }
            Err(e) => {
                warn!(pod = %pod, stream = stream_name, error = %e, "log stream error");
                break;
            }
        }
    }
}
