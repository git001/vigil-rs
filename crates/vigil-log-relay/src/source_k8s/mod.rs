//! Kubernetes pod log source — event-driven via kube::runtime::watcher.
//!
//! Uses the Kubernetes Watch API to react immediately to pod state changes
//! (no polling delay). A periodic reconcile loop restarts streams that were
//! closed by the API server (typically every ~5 minutes).
//!
//! Design:
//!   running_pods  — source of truth: pods known to be in Running phase
//!   active        — pods with a currently running stream JoinHandle
//!
//!   Watcher events update running_pods and start/stop streams immediately.
//!   The reconcile ticker (--watch-interval) cleans up finished handles and
//!   restarts streams for pods still in running_pods with no active task.

mod client;
mod stream;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use kube::{Api, runtime::watcher};
use regex::Regex;
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval};
use tracing::{info, warn};

use crate::{LineFilter, Liveness, cli::Cli};

// ---------------------------------------------------------------------------
// Main event loop
// ---------------------------------------------------------------------------

pub async fn run(
    cli: Cli,
    tx: mpsc::Sender<String>,
    liveness: Arc<Liveness>,
    filter: LineFilter,
) -> Result<()> {
    let filter = Arc::new(filter);
    let exclude_pod_re: Vec<Regex> = cli
        .exclude_pod
        .iter()
        .map(|p| {
            Regex::new(p).map_err(|e| anyhow::anyhow!("invalid --exclude-pod regex: {p}: {e}"))
        })
        .collect::<Result<_>>()?;

    let semaphore = if cli.max_log_requests > 0 {
        Some(Arc::new(tokio::sync::Semaphore::new(cli.max_log_requests)))
    } else {
        None
    };

    let kube_client = client::build(&cli).await?;
    let pods_api: Api<Pod> = Api::namespaced(kube_client.clone(), &cli.namespace);
    let namespace = Arc::new(cli.namespace.clone());
    let container = cli.container.map(Arc::new);

    let use_stream_param = !cli.no_stream_param && detect_stream_param_support(&kube_client).await;

    let watcher_config = if cli.pod_selector.is_empty() {
        watcher::Config::default()
    } else {
        watcher::Config::default().labels(&cli.pod_selector)
    };
    let mut pod_events = watcher(pods_api, watcher_config).boxed();

    // running_pods: pods known to be in Running phase (from watcher events)
    let mut running_pods: HashSet<String> = HashSet::new();
    // active: pods with a currently running stream task
    let mut active: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    // initialized: pods that have already had their first stream started.
    // First stream uses --tail-lines; reconnects use --since-seconds instead.
    let mut initialized: HashSet<String> = HashSet::new();

    let mut reconcile = interval(Duration::from_secs(cli.watch_interval));
    reconcile.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            event = pod_events.next() => {
                let event = match event {
                    Some(Ok(e))  => e,
                    Some(Err(e)) => {
                        warn!(error = %e, "pod watcher error — will reconnect");
                        continue;
                    }
                    None => break,
                };

                match event {
                    watcher::Event::Apply(pod) | watcher::Event::InitApply(pod) => {
                        let is_running = pod.status.as_ref()
                            .and_then(|s| s.phase.as_deref()) == Some("Running");
                        let name = match pod.metadata.name {
                            Some(n) => n,
                            None    => continue,
                        };

                        if is_running {
                            if exclude_pod_re.iter().any(|re| re.is_match(&name)) {
                                continue;
                            }
                            running_pods.insert(name.clone());
                            let (tail, since) = stream_params(&name, &initialized, cli.tail_lines, cli.since_seconds);
                            if stream::try_start(
                                &name, &mut active, &semaphore,
                                &kube_client, &namespace, &container,
                                tail, since, use_stream_param,
                                &filter, &tx,
                            ) {
                                initialized.insert(name.clone());
                            }
                        } else if running_pods.remove(&name)
                            && let Some(handle) = active.remove(&name)
                        {
                            info!(pod = %name, "pod no longer running — aborting stream");
                            handle.abort();
                        }
                    }

                    watcher::Event::Delete(pod) => {
                        if let Some(name) = pod.metadata.name {
                            running_pods.remove(&name);
                            initialized.remove(&name);
                            if let Some(handle) = active.remove(&name) {
                                info!(pod = %name, "pod deleted — aborting stream");
                                handle.abort();
                            }
                        }
                    }

                    watcher::Event::InitDone => {
                        info!(pods = running_pods.len(), "initial pod list complete");
                        liveness.tick();
                    }

                    watcher::Event::Init => {}
                }
            }

            _ = reconcile.tick() => {
                active.retain(|pod, handle| {
                    if handle.is_finished() {
                        info!(pod = %pod, "stream finished — scheduling restart");
                        false
                    } else {
                        true
                    }
                });

                let pods: Vec<String> = running_pods.iter().cloned().collect();
                for name in &pods {
                    let (tail, since) = stream_params(name, &initialized, cli.tail_lines, cli.since_seconds);
                    if stream::try_start(
                        name, &mut active, &semaphore,
                        &kube_client, &namespace, &container,
                        tail, since, use_stream_param,
                        &filter, &tx,
                    ) {
                        initialized.insert(name.clone());
                    }
                }

                liveness.tick();
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Detect whether the K8s API server supports the `stream` query parameter
/// for pod log endpoints (requires K8s ≥ 1.32).
async fn detect_stream_param_support(client: &kube::Client) -> bool {
    let info = match client.apiserver_version().await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "failed to detect K8s version — stream param disabled");
            return false;
        }
    };
    // Minor version may carry a suffix (e.g. "34+") — strip non-digit tail.
    let minor: u32 = info
        .minor
        .trim_end_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(0);
    let major: u32 = info.major.parse().unwrap_or(0);
    let supported = (major == 1 && minor >= 32) || major > 1;
    if supported {
        info!(
            k8s_version = %format!("{}.{}", info.major, info.minor),
            "K8s stream param supported — stdout/stderr separated"
        );
    } else {
        info!(
            k8s_version = %format!("{}.{}", info.major, info.minor),
            "K8s stream param not supported (< 1.32) — using combined output stream"
        );
    }
    supported
}

/// Returns (tail_lines, since_seconds) for a stream start.
/// First connect uses tail_lines; reconnects use since_seconds only.
fn stream_params(
    pod: &str,
    initialized: &HashSet<String>,
    tail_lines: i64,
    since_seconds: i64,
) -> (i64, i64) {
    if initialized.contains(pod) {
        (0, since_seconds)
    } else {
        (tail_lines, since_seconds)
    }
}
