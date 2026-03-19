// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use reqwest::Client as HttpClient;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{MissedTickBehavior, interval, timeout};
use tracing::{debug, info, warn};
use vigil_types::api::{CheckInfo, CheckStatus};
use vigil_types::plan::{CheckConfig, ExecCheck, ServiceConfig};

use crate::duration::parse_duration;
use crate::metrics::MetricsStore;
use crate::process_util::resolve_gid;
use crate::process_util::resolve_uid;

const DEFAULT_PERIOD: Duration = Duration::from_secs(10);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_THRESHOLD: u32 = 3;
const DEFAULT_CHECK_DELAY: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

pub enum Cmd {
    GetStatus(oneshot::Sender<CheckInfo>),
    Shutdown,
}

#[allow(dead_code)] // fields used in future on-check-failure wiring
pub struct CheckEvent {
    pub check: String,
    pub status: CheckStatus,
}

pub struct Handle {
    pub tx: mpsc::Sender<Cmd>,
}

pub fn spawn(
    name: String,
    config: CheckConfig,
    service_configs: Arc<IndexMap<String, ServiceConfig>>,
    event_tx: mpsc::Sender<CheckEvent>,
    metrics: Arc<MetricsStore>,
) -> Handle {
    let (tx, rx) = mpsc::channel(16);
    tokio::spawn(run(name, config, service_configs, rx, event_tx, metrics));
    Handle { tx }
}

// ---------------------------------------------------------------------------
// Actor loop
// ---------------------------------------------------------------------------

async fn run(
    name: String,
    config: CheckConfig,
    service_configs: Arc<IndexMap<String, ServiceConfig>>,
    mut rx: mpsc::Receiver<Cmd>,
    event_tx: mpsc::Sender<CheckEvent>,
    metrics: Arc<MetricsStore>,
) {
    let period = config
        .period
        .as_deref()
        .and_then(|s| parse_duration(s).ok())
        .unwrap_or(DEFAULT_PERIOD);

    let timeout_dur = config
        .timeout
        .as_deref()
        .and_then(|s| parse_duration(s).ok())
        .unwrap_or(DEFAULT_TIMEOUT)
        .min(period);

    let threshold = config.threshold.unwrap_or(DEFAULT_THRESHOLD);

    let http_client = Arc::new(
        HttpClient::builder()
            .timeout(timeout_dur)
            .build()
            .unwrap_or_default(),
    );

    // Wait for the initial delay before the first check (default: 3s).
    // Responds to GetStatus (reports "up, 0 failures") and Shutdown during the wait.
    let delay_dur = config
        .delay
        .as_deref()
        .and_then(|s| parse_duration(s).ok())
        .unwrap_or(DEFAULT_CHECK_DELAY);
    {
        let deadline = tokio::time::Instant::now() + delay_dur;
        loop {
            tokio::select! {
                biased;
                cmd = rx.recv() => match cmd {
                    None | Some(Cmd::Shutdown) => return,
                    Some(Cmd::GetStatus(reply)) => {
                        let _ = reply.send(CheckInfo {
                            name: name.clone(),
                            level: config.level,
                            status: CheckStatus::Up,
                            failures: 0,
                            threshold: config.threshold.unwrap_or(DEFAULT_THRESHOLD),
                        });
                    }
                },
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
    }

    let mut failures: u32 = 0;
    let mut status = CheckStatus::Up;
    // Initialise check_up=1 before the first tick
    metrics.set_check_up(&name, true);

    let mut tick = interval(period);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;

            cmd = rx.recv() => match cmd {
                None | Some(Cmd::Shutdown) => break,
                Some(Cmd::GetStatus(reply)) => {
                    let _ = reply.send(CheckInfo {
                        name: name.clone(),
                        level: config.level,
                        status,
                        failures,
                        threshold,
                    });
                }
            },

            _ = tick.tick() => {
                let ok = perform(&config, timeout_dur, &http_client, &service_configs).await;
                if ok {
                    metrics.record_check_success(&name);
                    if status == CheckStatus::Down {
                        info!(check = %name, "check recovered");
                        status = CheckStatus::Up;
                        metrics.set_check_up(&name, true);
                        let _ = event_tx.send(CheckEvent { check: name.clone(), status }).await;
                    }
                    failures = 0;
                } else {
                    metrics.record_check_failure(&name);
                    failures += 1;
                    warn!(check = %name, failures, threshold, "check failed");
                    if failures >= threshold && status == CheckStatus::Up {
                        info!(check = %name, "check is down");
                        status = CheckStatus::Down;
                        metrics.set_check_up(&name, false);
                        let _ = event_tx.send(CheckEvent { check: name.clone(), status }).await;
                    }
                }
            }
        }
    }

    debug!(check = %name, "check actor shut down");
}

// ---------------------------------------------------------------------------
// Check implementations
// ---------------------------------------------------------------------------

async fn perform(
    config: &CheckConfig,
    timeout_dur: Duration,
    http: &HttpClient,
    service_configs: &IndexMap<String, ServiceConfig>,
) -> bool {
    if let Some(h) = &config.http {
        return http_check(http, &h.url, timeout_dur).await;
    }
    if let Some(t) = &config.tcp {
        let host = t.host.as_deref().unwrap_or("localhost");
        return tcp_check(host, t.port, timeout_dur).await;
    }
    if let Some(e) = &config.exec {
        return exec_check(e, timeout_dur, service_configs).await;
    }
    true
}

async fn http_check(client: &HttpClient, url: &str, timeout_dur: Duration) -> bool {
    match timeout(timeout_dur, client.get(url).send()).await {
        Ok(Ok(resp)) => resp.status().is_success(),
        _ => false,
    }
}

async fn tcp_check(host: &str, port: u16, timeout_dur: Duration) -> bool {
    let addr = format!("{}:{}", host, port);
    matches!(
        timeout(timeout_dur, tokio::net::TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

async fn exec_check(
    exec: &ExecCheck,
    timeout_dur: Duration,
    service_configs: &IndexMap<String, ServiceConfig>,
) -> bool {
    // Resolve service-context: inherit env/user/group/working-dir from the
    // named service, then let check-specific settings override.
    let ctx_svc = exec
        .service_context
        .as_deref()
        .and_then(|n| service_configs.get(n));

    // Build effective environment: service env first, check env on top.
    let mut env: IndexMap<String, String> = ctx_svc
        .map(|s| s.environment.clone())
        .unwrap_or_default();
    env.extend(exec.environment.iter().map(|(k, v)| (k.clone(), v.clone())));

    // Effective user/group: check setting wins, fall back to service context.
    let eff_user = exec.user.as_deref().or_else(|| ctx_svc.and_then(|s| s.user.as_deref()));
    let eff_user_id = exec.user_id.or_else(|| ctx_svc.and_then(|s| s.user_id));
    let eff_group =
        exec.group.as_deref().or_else(|| ctx_svc.and_then(|s| s.group.as_deref()));
    let eff_group_id = exec.group_id.or_else(|| ctx_svc.and_then(|s| s.group_id));
    let eff_working_dir = exec
        .working_dir
        .as_deref()
        .or_else(|| ctx_svc.and_then(|s| s.working_dir.as_deref()));

    // Resolve uid/gid (fail-safe: log and skip on error).
    let uid = match resolve_uid(eff_user, eff_user_id) {
        Ok(u) => u,
        Err(e) => {
            warn!(%e, "exec check: failed to resolve user");
            return false;
        }
    };
    let gid = match resolve_gid(eff_group, eff_group_id) {
        Ok(g) => g,
        Err(e) => {
            warn!(%e, "exec check: failed to resolve group");
            return false;
        }
    };

    // Clone everything needed into the spawned future.
    let command = exec.command.clone();
    let working_dir = eff_working_dir.map(str::to_owned);

    match timeout(timeout_dur, async move {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", &command]);

        if !env.is_empty() {
            cmd.envs(env.iter());
        }
        if let Some(dir) = &working_dir {
            cmd.current_dir(dir);
        }
        if uid.is_some() || gid.is_some() {
            unsafe {
                cmd.pre_exec(move || {
                    if let Some(g) = gid {
                        nix::unistd::setgid(g).map_err(|e| {
                            std::io::Error::new(
                                std::io::ErrorKind::PermissionDenied,
                                e.to_string(),
                            )
                        })?;
                    }
                    if let Some(u) = uid {
                        nix::unistd::setuid(u).map_err(|e| {
                            std::io::Error::new(
                                std::io::ErrorKind::PermissionDenied,
                                e.to_string(),
                            )
                        })?;
                    }
                    Ok(())
                });
            }
        }

        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        cmd.spawn().ok()?.wait().await.ok()
    })
    .await
    {
        Ok(Some(status)) => status.success(),
        _ => false,
    }
}
