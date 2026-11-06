//! Per-pod log streaming task.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    stream_param: &Arc<AtomicBool>,
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

    let handle = if stream_param.load(Ordering::Relaxed) {
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
        // Fallback if the API server rejects `stream` (feature gate off).
        let url_combined = log_url(
            namespace,
            pod,
            container_key,
            tail_lines,
            since_seconds,
            None,
        );
        let stream_param = Arc::clone(stream_param);
        let client = kube_client.clone();
        let pod_str = pod.to_owned();
        let ns = Arc::clone(namespace);
        let cname_out = container_name.clone();
        let cname_err = container_name.clone();
        let cname_out_fallback = container_name.clone();
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
                client.clone(),
                url_err,
                pod_str.clone(),
                Arc::clone(&ns),
                cname_err,
                Arc::clone(&filter_err),
                tx_err.clone(),
                "stderr",
            ));
            // When either stream closes (K8s API server timeout), abort the sibling.
            let first = tokio::select! {
                r = &mut h_out => { h_err.abort(); r }
                r = &mut h_err => { h_out.abort(); r }
            };
            if matches!(first, Ok(StreamEnd::StreamParamRejected)) {
                if stream_param.swap(false, Ordering::Relaxed) {
                    warn!(
                        "the API server rejects the `stream` parameter (feature gate \
                         PodLogsQuerySplitStreams off); using one combined stream \
                         (stream: \"output\") from now on"
                    );
                }
                stream_pod(
                    client,
                    url_combined,
                    pod_str,
                    ns,
                    cname_out_fallback,
                    filter_err,
                    tx_err,
                    "output",
                )
                .await;
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

/// How a log stream ended.
#[derive(Debug, PartialEq, Eq)]
enum StreamEnd {
    /// EOF, an error or a failed request; the reconcile loop restarts it.
    Closed,
    /// The API server does not accept the `stream` query parameter.
    StreamParamRejected,
}

/// `stream=Stdout|Stderr` needs the PodLogsQuerySplitStreams feature gate,
/// which is off by default even on recent Kubernetes versions; the API
/// server then answers 422 "stream: Forbidden: may not be specified".
fn is_stream_param_rejected(e: &kube::Error) -> bool {
    matches!(e, kube::Error::Api(r) if r.code == 422 && r.message.contains("stream: Forbidden"))
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
) -> StreamEnd {
    let container_str = container.as_deref().map(|s| s.as_str());

    let req = match http::Request::get(&url).body(vec![]) {
        Ok(r) => r,
        Err(e) => {
            warn!(pod = %pod, stream = stream_name, error = %e, "failed to build log request");
            return StreamEnd::Closed;
        }
    };

    let byte_stream = match client.request_stream(req).await {
        Ok(s) => s,
        Err(e) if is_stream_param_rejected(&e) => return StreamEnd::StreamParamRejected,
        Err(e) => {
            warn!(pod = %pod, stream = stream_name, error = %e, "failed to open log stream");
            return StreamEnd::Closed;
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
                return StreamEnd::Closed;
            }
            Err(e) => {
                warn!(pod = %pod, stream = stream_name, error = %e, "log stream error");
                return StreamEnd::Closed;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_error(code: u16, message: &str) -> kube::Error {
        kube::Error::Api(kube::core::ErrorResponse {
            status: "Failure".into(),
            message: message.into(),
            reason: "Invalid".into(),
            code,
        })
    }

    #[test]
    fn rejected_stream_param_is_recognised() {
        // The answer of a 1.35 API server without the feature gate.
        let e = api_error(
            422,
            r#"PodLogOptions "logger-a" is invalid: stream: Forbidden: may not be specified"#,
        );
        assert!(is_stream_param_rejected(&e));
        // Other errors keep the separate streams.
        assert!(!is_stream_param_rejected(&api_error(
            422,
            "tailLines: Invalid value"
        )));
        assert!(!is_stream_param_rejected(&api_error(
            403,
            "stream: Forbidden"
        )));
    }

    #[test]
    fn log_url_adds_only_the_given_parameters() {
        assert_eq!(
            log_url("ns", "p", "", 0, 0, None),
            "/api/v1/namespaces/ns/pods/p/log?follow=true&timestamps=true"
        );
        assert_eq!(
            log_url("ns", "p", "app", 5, 10, Some("Stderr")),
            "/api/v1/namespaces/ns/pods/p/log?follow=true&timestamps=true\
             &container=app&tailLines=5&stream=Stderr"
        );
        assert_eq!(
            log_url("ns", "p", "", 0, 10, None),
            "/api/v1/namespaces/ns/pods/p/log?follow=true&timestamps=true&sinceSeconds=10"
        );
    }
}
