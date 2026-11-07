//! Starting the log stream tasks of a pod container.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kube::Client;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::reader::{StreamEnd, stream_pod};
use crate::LineFilter;

/// What every log stream of the Kubernetes source needs.
#[derive(Clone)]
pub struct StreamCtx {
    pub client: Client,
    pub namespace: Arc<String>,
    /// `--max-log-requests`: one permit per (pod, container) stream.
    pub semaphore: Option<Arc<tokio::sync::Semaphore>>,
    /// Whether to ask for separate stdout and stderr streams; the first
    /// stream the API server rejects turns it off for the whole process.
    pub stream_param: Arc<AtomicBool>,
    pub filter: Arc<LineFilter>,
    pub tx: mpsc::Sender<String>,
}

/// Start a stream task if none is active for this (pod, container_key)
/// pair. container_key "" means no &container= param (K8s picks the
/// default). Returns true if a new task was spawned.
pub fn try_start(
    ctx: &StreamCtx,
    active: &mut HashMap<(String, String), tokio::task::JoinHandle<()>>,
    pod: &str,
    container_key: &str,
    tail_lines: i64,
    since_seconds: i64,
) -> bool {
    let key = (pod.to_owned(), container_key.to_owned());
    if active.contains_key(&key) {
        return false;
    }

    let permit = match &ctx.semaphore {
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

    let url = |stream| {
        log_url(
            &ctx.namespace,
            pod,
            container_key,
            tail_lines,
            since_seconds,
            stream,
        )
    };
    let ctx = ctx.clone();
    let pod = pod.to_owned();
    let handle = if ctx.stream_param.load(Ordering::Relaxed) {
        // K8s ≥ 1.32: request stdout and stderr as separate streams.
        let url_out = url(Some("Stdout"));
        let url_err = url(Some("Stderr"));
        // Fallback if the API server rejects `stream` (feature gate off).
        let url_combined = url(None);
        tokio::spawn(async move {
            let _permit = permit;
            let mut h_out = tokio::spawn(stream_pod(
                ctx.clone(),
                pod.clone(),
                container_name.clone(),
                url_out,
                "stdout",
            ));
            let mut h_err = tokio::spawn(stream_pod(
                ctx.clone(),
                pod.clone(),
                container_name.clone(),
                url_err,
                "stderr",
            ));
            // When either stream closes (K8s API server timeout), abort the sibling.
            let first = tokio::select! {
                r = &mut h_out => { h_err.abort(); r }
                r = &mut h_err => { h_out.abort(); r }
            };
            if matches!(first, Ok(StreamEnd::StreamParamRejected)) {
                if ctx.stream_param.swap(false, Ordering::Relaxed) {
                    warn!(
                        "the API server rejects the `stream` parameter (feature gate \
                         PodLogsQuerySplitStreams off); using one combined stream \
                         (stream: \"output\") from now on"
                    );
                }
                stream_pod(ctx, pod, container_name, url_combined, "output").await;
            }
        })
    } else {
        // K8s < 1.32: single combined stream, labeled "output".
        let url = url(None);
        tokio::spawn(async move {
            let _permit = permit;
            stream_pod(ctx, pod, container_name, url, "output").await;
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

#[cfg(test)]
mod tests {
    use super::*;

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
