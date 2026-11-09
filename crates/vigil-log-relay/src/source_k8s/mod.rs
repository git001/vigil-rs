//! Kubernetes pod log source — event-driven via kube::runtime::watcher.
//!
//! Uses the Kubernetes Watch API to react immediately to pod state changes
//! (no polling delay). A periodic reconcile loop restarts streams that were
//! closed by the API server (typically every ~5 minutes).
//!
//! Design: [`pods::Pods`] holds the pods known to be Running and their
//! stream tasks. Watcher events update it and start or stop streams
//! immediately. The reconcile ticker (--watch-interval) cleans up finished
//! tasks and restarts streams for pods still Running with no active task.

mod client;
mod pods;
mod reader;
mod records;
mod selector;
mod stream;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
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
use pods::Pods;
use selector::ContainerSelector;
use stream::StreamCtx;

// ---------------------------------------------------------------------------
// Main event loop
// ---------------------------------------------------------------------------

pub async fn run(
    cli: Cli,
    tx: mpsc::Sender<String>,
    liveness: Arc<Liveness>,
    filter: LineFilter,
    join: crate::join::JoinConfig,
) -> Result<()> {
    let filter = Arc::new(filter);
    let exclude_pod_re: Vec<Regex> = cli
        .k8s
        .exclude_pod
        .iter()
        .map(|p| {
            Regex::new(p).map_err(|e| anyhow::anyhow!("invalid --exclude-pod regex: {p}: {e}"))
        })
        .collect::<Result<_>>()?;

    let semaphore = if cli.k8s.max_log_requests > 0 {
        Some(Arc::new(tokio::sync::Semaphore::new(
            cli.k8s.max_log_requests,
        )))
    } else {
        None
    };

    let kube_client = client::build(&cli).await?;
    let pods_api: Api<Pod> = Api::namespaced(kube_client.clone(), &cli.k8s.namespace);
    let selector = ContainerSelector::from_cli(cli.k8s.container.as_deref());

    // Shared by all streams: the first one the API server rejects turns it
    // off for the rest of the process.
    let stream_param = Arc::new(AtomicBool::new(
        !cli.k8s.no_stream_param && detect_stream_param_support(&kube_client).await,
    ));

    let mut pods = Pods::new(
        StreamCtx {
            client: kube_client,
            namespace: Arc::new(cli.k8s.namespace.clone()),
            semaphore,
            stream_param,
            filter,
            tx,
            lines: Arc::new(join),
        },
        cli.k8s.tail_lines,
        cli.k8s.since_seconds,
    );

    let watcher_config = if cli.k8s.pod_selector.is_empty() {
        watcher::Config::default()
    } else {
        watcher::Config::default().labels(&cli.k8s.pod_selector)
    };
    let mut pod_events = watcher(pods_api, watcher_config).boxed();

    let mut reconcile = interval(Duration::from_secs(cli.k8s.watch_interval));
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
                        let Some(name) = pod.metadata.name.clone() else {
                            continue;
                        };

                        if is_running {
                            if exclude_pod_re.iter().any(|re| re.is_match(&name)) {
                                continue;
                            }
                            pods.running(name, selector.keys_for_pod(&pod));
                        } else if let Some(aborted) = pods.not_running(&name)
                            && aborted > 0
                        {
                            info!(pod = %name, "pod no longer running — aborting streams");
                        }
                    }

                    watcher::Event::Delete(pod) => {
                        if let Some(name) = pod.metadata.name {
                            pods.deleted(&name);
                        }
                    }

                    watcher::Event::InitDone => {
                        info!(pods = pods.running_count(), "initial pod list complete");
                        liveness.tick();
                    }

                    watcher::Event::Init => {}
                }
            }

            _ = reconcile.tick() => {
                pods.reconcile();
                liveness.tick();
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Whether to try the `stream` query parameter for pod logs (K8s ≥ 1.32).
/// It also needs a feature gate the version does not tell; streams fall
/// back to one combined stream if the API server rejects it.
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
            "K8s ≥ 1.32 — trying separate stdout/stderr streams"
        );
    } else {
        info!(
            k8s_version = %format!("{}.{}", info.major, info.minor),
            "K8s stream param not supported (< 1.32) — using combined output stream"
        );
    }
    supported
}
