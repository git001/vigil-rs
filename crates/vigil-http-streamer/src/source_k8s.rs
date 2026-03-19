//! Kubernetes pod log source.
//!
//! Polls the K8s API for Running pods every --watch-interval seconds,
//! maintains one async task per pod that follows its log stream, and sends
//! each log line as an ndjson object to the shared TCP sink channel.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{ListParams, LogParams},
    Api, Client,
};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{info, warn};

use crate::{Cli, Liveness};

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
// Main K8s watch loop
// ---------------------------------------------------------------------------

pub async fn run(cli: Cli, tx: mpsc::Sender<String>, liveness: Arc<Liveness>) -> Result<()> {
    let pod_selector = if cli.pod_selector.is_empty() {
        None
    } else {
        Some(cli.pod_selector.clone())
    };

    let client = Client::try_default()
        .await
        .context("failed to create Kubernetes client — is this running in-cluster?")?;
    let pods_api: Api<Pod> = Api::namespaced(client, &cli.namespace);
    let namespace = Arc::new(cli.namespace.clone());

    let mut active: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();

    let mut ticker = interval(Duration::from_secs(cli.watch_interval));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;

        let current = list_running_pods(&pods_api, pod_selector.as_deref()).await;

        // Prune tasks for gone pods and naturally finished streams
        active.retain(|pod, handle| {
            if !current.contains(pod) {
                info!(pod = %pod, "pod gone — aborting stream");
                handle.abort();
                false
            } else {
                !handle.is_finished()
            }
        });

        // Start streams for new or restarted pods
        for pod in &current {
            if !active.contains_key(pod.as_str()) {
                info!(pod = %pod, "starting log stream");
                let handle = tokio::spawn(stream_pod(
                    pods_api.clone(),
                    pod.clone(),
                    Arc::clone(&namespace),
                    tx.clone(),
                ));
                active.insert(pod.clone(), handle);
            }
        }

        // Signal liveness: the healthcheck HTTP server reads this to answer /healthz.
        // If this tick stops happening (loop stuck), /healthz returns 503 and
        // vigild restarts the service.
        liveness.tick();
    }
}

// ---------------------------------------------------------------------------
// Pod listing
// ---------------------------------------------------------------------------

async fn list_running_pods(pods: &Api<Pod>, selector: Option<&str>) -> Vec<String> {
    let lp = match selector {
        Some(sel) => ListParams::default().labels(sel),
        None => ListParams::default(),
    };
    match pods.list(&lp).await {
        Ok(list) => list
            .items
            .into_iter()
            .filter(|p| {
                p.status
                    .as_ref()
                    .and_then(|s| s.phase.as_deref())
                    == Some("Running")
            })
            .filter_map(|p| p.metadata.name)
            .collect(),
        Err(e) => {
            warn!(error = %e, "failed to list pods — retrying next cycle");
            vec![]
        }
    }
}

// ---------------------------------------------------------------------------
// Per-pod streaming task
// ---------------------------------------------------------------------------

async fn stream_pod(
    pods: Api<Pod>,
    pod: String,
    namespace: Arc<String>,
    tx: mpsc::Sender<String>,
) {
    let params = LogParams {
        follow: true,
        timestamps: true,
        ..Default::default()
    };

    let byte_stream = match pods.log_stream(&pod, &params).await {
        Ok(s) => s,
        Err(e) => {
            warn!(pod = %pod, error = %e, "failed to open log stream");
            return;
        }
    };

    // kube returns futures_io::AsyncBufRead; .compat() bridges it to tokio::AsyncRead
    let mut lines = BufReader::new(byte_stream.compat()).lines();

    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                // K8s timestamp-prefixed lines: "2026-03-18T11:23:45.123Z <message>"
                let (ts, msg) = match line.find(' ') {
                    Some(i) => (&line[..i], &line[i + 1..]),
                    None => (line.as_str(), ""),
                };
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
