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
use vigil_types::plan::{CheckConfig, ExecCheck, HttpCheck, ServiceConfig};

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
        return http_check(http, h, timeout_dur).await;
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

async fn http_check(client: &HttpClient, check: &HttpCheck, timeout_dur: Duration) -> bool {
    // Build a per-check client only when TLS options require it; reuse the
    // shared client for the common case (no insecure / no custom CA).
    let owned;
    let effective_client: &HttpClient = if check.insecure || check.ca.is_some() {
        let mut b = HttpClient::builder()
            .timeout(timeout_dur)
            .danger_accept_invalid_certs(check.insecure);
        if let Some(ca_path) = &check.ca {
            match load_pem_chain(ca_path) {
                Ok(certs) => {
                    for cert in certs {
                        b = b.add_root_certificate(cert);
                    }
                }
                Err(e) => {
                    warn!(path = %ca_path.display(), error = %e, "http check: failed to load ca cert");
                    return false;
                }
            }
        }
        owned = b.build().unwrap_or_default();
        &owned
    } else {
        client
    };

    let mut req = effective_client.get(&check.url);
    for (name, value) in &check.headers {
        req = req.header(name.as_str(), value.as_str());
    }
    match timeout(timeout_dur, req.send()).await {
        Ok(Ok(resp)) => resp.status().is_success(),
        _ => false,
    }
}

/// Load all PEM certificate blocks from `path` as reqwest `Certificate` objects.
/// Supports chain files with multiple concatenated `-----BEGIN CERTIFICATE-----` blocks.
fn load_pem_chain(path: &std::path::Path) -> anyhow::Result<Vec<reqwest::Certificate>> {
    let pem = std::fs::read_to_string(path)?;
    let mut certs = Vec::new();
    let mut block = String::new();
    for line in pem.lines() {
        block.push_str(line);
        block.push('\n');
        if line.trim() == "-----END CERTIFICATE-----" {
            certs.push(
                reqwest::Certificate::from_pem(block.as_bytes())
                    .map_err(|e| anyhow::anyhow!("invalid PEM block: {e}"))?,
            );
            block.clear();
        }
    }
    if certs.is_empty() {
        anyhow::bail!("no certificates found in {}", path.display());
    }
    Ok(certs)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get, http::StatusCode};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper_util::rt::TokioIo;
    use indexmap::IndexMap;
    use std::time::Duration;
    use tower::Service;
    use vigil_types::plan::HttpCheck;

    // ------------------------------------------------------------------
    // Test server helpers
    // ------------------------------------------------------------------

    /// Spawn a plain HTTP server on a random port. Returns the port.
    async fn spawn_http_server(app: Router) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        port
    }

    /// Spawn a TLS server on a random port.
    /// Returns `(port, cert_der)` — the cert DER can be converted to PEM
    /// for use as a CA cert in the `ca:` check option.
    async fn spawn_tls_server(app: Router) -> (u16, Vec<u8>) {
        let (cert_ders, key_der) = crate::tls::generate_self_signed(&["localhost"]).unwrap();
        let cert_der = cert_ders[0].clone();
        let acceptor = crate::tls::acceptor_from_der(cert_ders, key_der).unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            loop {
                let Ok((stream, _peer)) = listener.accept().await else { break };
                let acceptor = acceptor.clone();
                let app = app.clone();
                tokio::spawn(async move {
                    match acceptor.accept(stream).await {
                        Err(_) => {}
                        Ok(tls_stream) => {
                            let io = TokioIo::new(tls_stream);
                            let svc = hyper::service::service_fn(
                                move |req: hyper::Request<Incoming>| {
                                    let mut r = app.clone();
                                    async move { r.call(req.map(axum::body::Body::new)).await }
                                },
                            );
                            let _ = http1::Builder::new().serve_connection(io, svc).await;
                        }
                    }
                });
            }
        });

        (port, cert_der)
    }

    fn init_crypto() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        });
    }

    fn make_check(url: String) -> HttpCheck {
        HttpCheck { url, headers: IndexMap::new(), insecure: false, ca: None }
    }

    fn shared_client() -> HttpClient {
        HttpClient::builder().timeout(Duration::from_secs(5)).build().unwrap()
    }

    // ------------------------------------------------------------------
    // Plain HTTP
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn http_2xx_passes() {
        let app = Router::new().route("/ok", get(|| async { StatusCode::OK }));
        let port = spawn_http_server(app).await;
        let check = make_check(format!("http://127.0.0.1:{port}/ok"));
        assert!(http_check(&shared_client(), &check, Duration::from_secs(5)).await);
    }

    #[tokio::test]
    async fn http_4xx_fails() {
        let app = Router::new().route("/nf", get(|| async { StatusCode::NOT_FOUND }));
        let port = spawn_http_server(app).await;
        let check = make_check(format!("http://127.0.0.1:{port}/nf"));
        assert!(!http_check(&shared_client(), &check, Duration::from_secs(5)).await);
    }

    #[tokio::test]
    async fn http_5xx_fails() {
        let app =
            Router::new().route("/err", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }));
        let port = spawn_http_server(app).await;
        let check = make_check(format!("http://127.0.0.1:{port}/err"));
        assert!(!http_check(&shared_client(), &check, Duration::from_secs(5)).await);
    }

    #[tokio::test]
    async fn custom_headers_are_forwarded() {
        // Server returns 200 only when the X-Vigil-Test header equals "yes".
        let app = Router::new().route(
            "/hdr",
            get(|hdrs: axum::http::HeaderMap| async move {
                if hdrs.get("x-vigil-test").and_then(|v| v.to_str().ok()) == Some("yes") {
                    StatusCode::OK
                } else {
                    StatusCode::UNAUTHORIZED
                }
            }),
        );
        let port = spawn_http_server(app).await;
        let url = format!("http://127.0.0.1:{port}/hdr");

        // Without the header the server returns 401 → check fails.
        let without = make_check(url.clone());
        assert!(!http_check(&shared_client(), &without, Duration::from_secs(5)).await);

        // With the header the server returns 200 → check passes.
        let mut with_hdr = make_check(url);
        with_hdr.headers.insert("x-vigil-test".to_string(), "yes".to_string());
        assert!(http_check(&shared_client(), &with_hdr, Duration::from_secs(5)).await);
    }

    // ------------------------------------------------------------------
    // TLS
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn self_signed_rejected_by_default() {
        init_crypto();
        let app = Router::new().route("/", get(|| async { StatusCode::OK }));
        let (port, _cert_der) = spawn_tls_server(app).await;
        let check = make_check(format!("https://localhost:{port}/"));
        // No insecure flag, no CA → reqwest rejects the self-signed cert.
        assert!(!http_check(&shared_client(), &check, Duration::from_secs(5)).await);
    }

    #[tokio::test]
    async fn insecure_accepts_self_signed() {
        init_crypto();
        let app = Router::new().route("/", get(|| async { StatusCode::OK }));
        let (port, _cert_der) = spawn_tls_server(app).await;
        let mut check = make_check(format!("https://localhost:{port}/"));
        check.insecure = true;
        assert!(http_check(&shared_client(), &check, Duration::from_secs(5)).await);
    }

    #[tokio::test]
    async fn ca_cert_accepts_self_signed() {
        init_crypto();
        let app = Router::new().route("/", get(|| async { StatusCode::OK }));
        let (port, cert_der) = spawn_tls_server(app).await;

        // Write the server cert as PEM to a temp file so load_pem_chain() can read it.
        let ca_pem = crate::tls::cert_to_pem(&cert_der);
        let tmp_path = std::env::temp_dir().join(format!("vigil-test-ca-{port}.pem"));
        std::fs::write(&tmp_path, &ca_pem).unwrap();

        let mut check = make_check(format!("https://localhost:{port}/"));
        check.ca = Some(tmp_path.clone());
        let result = http_check(&shared_client(), &check, Duration::from_secs(5)).await;
        let _ = std::fs::remove_file(&tmp_path);
        assert!(result);
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
