//! vigil-http-streamer — forward ndjson log streams to a TCP sink.
//!
//! Source modes (mutually exclusive):
//!
//!   --kubernetes
//!       Watch Running pods in a Kubernetes namespace via the K8s API.
//!
//!   --source-socket PATH  [--source-path /v1/logs/follow?format=ndjson]
//!       Stream from a Unix-domain socket (e.g. vigild local socket).
//!
//!   --source-url URL
//!       Stream from an HTTP/HTTPS URL (e.g. vigild TLS API).
//!
//! Output: newline-delimited JSON (ndjson) to a TCP listener.

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::Parser;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use tracing::info;

mod healthcheck;
mod source_http;
mod source_k8s;
mod tcp_sink;

pub use healthcheck::Liveness;
pub use source_http::ReconnectConfig;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name    = "vigil-http-streamer",
    about   = "Forward ndjson from Kubernetes pods or HTTP endpoints to a TCP sink",
    next_help_heading = "Source"
)]
pub struct Cli {
    // ---- Source (exactly one required) ------------------------------------

    /// Watch Running pods via the Kubernetes API and stream their logs.
    /// Requires an in-cluster service account (KUBERNETES_SERVICE_HOST).
    #[arg(long, help_heading = "Source")]
    pub kubernetes: bool,

    /// Stream ndjson from an HTTP/HTTPS URL (cannot combine with --source-socket).
    #[arg(long, value_name = "URL",
          conflicts_with_all = ["kubernetes", "source_socket"],
          help_heading = "Source")]
    pub source_url: Option<String>,

    /// Stream ndjson via a Unix-domain socket (cannot combine with --source-url).
    #[arg(long, value_name = "PATH",
          conflicts_with_all = ["kubernetes", "source_url"],
          help_heading = "Source")]
    pub source_socket: Option<PathBuf>,

    /// HTTP path to request over --source-socket.
    #[arg(long, value_name = "PATH",
          default_value = "/v1/logs/follow?format=ndjson",
          help_heading = "Source")]
    pub source_path: String,

    // ---- Kubernetes -------------------------------------------------------

    /// Namespace to watch.
    #[arg(long, env = "NAMESPACE", default_value = "default",
          help_heading = "Kubernetes")]
    pub namespace: String,

    /// Label selector, e.g. "app=myapp".
    #[arg(long, env = "POD_SELECTOR", default_value = "",
          help_heading = "Kubernetes")]
    pub pod_selector: String,

    /// Seconds between pod-list refreshes.
    #[arg(long, env = "WATCH_INTERVAL", default_value = "30",
          help_heading = "Kubernetes")]
    pub watch_interval: u64,

    // ---- TCP Sink ---------------------------------------------------------

    /// Sink host. Output is ndjson only (one JSON object per line).
    #[arg(long, env = "TCP_SINK_HOST", default_value = "127.0.0.1",
          help_heading = "TCP Sink")]
    pub tcp_sink_host: String,

    /// Sink port. Compatible with Filebeat / Fluent Bit / Logstash tcp input.
    #[arg(long, env = "TCP_SINK_PORT", default_value = "5170",
          help_heading = "TCP Sink")]
    pub tcp_sink_port: u16,

    // ---- Source Reconnect -------------------------------------------------

    /// Initial delay in ms; doubles each retry, capped at --reconnect-max.
    ///
    /// Triggers: connection refused, timeout, HTTP non-2xx, read error.
    /// Clean stream EOF resets the counter and delay.
    #[arg(long, env = "RECONNECT_DELAY", default_value = "500",
          value_name = "MS",
          help_heading = "Source Reconnect")]
    pub reconnect_delay: u64,

    /// Backoff ceiling in ms.
    #[arg(long, env = "RECONNECT_MAX", default_value = "30000",
          value_name = "MS",
          help_heading = "Source Reconnect")]
    pub reconnect_max: u64,

    /// Max consecutive failures before exit (0 = unlimited).
    ///
    /// vigild then restarts the process via on-failure: restart.
    /// Clean EOF does not count.
    #[arg(long, env = "RECONNECT_RETRIES", default_value = "0",
          value_name = "N",
          help_heading = "Source Reconnect")]
    pub reconnect_retries: u64,

    // ---- Health Check -----------------------------------------------------

    /// Address serving GET /healthz → 200 ok / 503 stale.
    #[arg(long, env = "HEALTHCHECK", default_value = "127.0.0.1:9091",
          value_name = "HOST:PORT",
          help_heading = "Health Check")]
    pub healthcheck: String,

    /// Seconds without a tick before /healthz returns 503 (≥ 3× --watch-interval).
    ///
    /// Kubernetes: tick per watch cycle.
    /// HTTP modes: background tick every 30 s.
    #[arg(long, env = "HEALTHCHECK_MAX_AGE", default_value = "90",
          value_name = "SECS",
          help_heading = "Health Check")]
    pub healthcheck_max_age: u64,
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .init();

    let cli = Cli::parse();

    let source_count = usize::from(cli.kubernetes)
        + usize::from(cli.source_url.is_some())
        + usize::from(cli.source_socket.is_some());
    if source_count == 0 {
        bail!("specify exactly one source: --kubernetes, --source-url, or --source-socket");
    }

    let addr = format!("{}:{}", cli.tcp_sink_host, cli.tcp_sink_port);
    let (tx, rx) = mpsc::channel::<String>(8192);

    // TCP sink — single task owning the connection
    tokio::spawn(tcp_sink::run(addr.clone(), rx));

    // Healthcheck HTTP server
    let liveness = Liveness::new(cli.healthcheck_max_age);
    tokio::spawn(healthcheck::serve(cli.healthcheck.clone(), liveness.clone()));

    // Reconnect config for HTTP source modes
    let reconnect = ReconnectConfig {
        initial_delay_ms: cli.reconnect_delay,
        max_delay_ms:     cli.reconnect_max,
        max_retries:      cli.reconnect_retries,
    };

    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    if cli.kubernetes {
        info!(
            namespace  = %cli.namespace,
            selector   = cli.pod_selector.as_str(),
            tcp        = %addr,
            interval_s = cli.watch_interval,
            healthcheck = %cli.healthcheck,
            "source: kubernetes pod logs",
        );
        let source = tokio::spawn(source_k8s::run(cli, tx, liveness));
        tokio::select! {
            res = source       => { res??; }
            _ = sigterm.recv() => { info!("received SIGTERM"); }
            _ = sigint.recv()  => { info!("received SIGINT");  }
        }
    } else if let Some(url) = cli.source_url.clone() {
        info!(url = %url, tcp = %addr, healthcheck = %cli.healthcheck, "source: http url");
        let source = tokio::spawn(source_http::run_url(url, tx, liveness, reconnect));
        tokio::select! {
            res = source       => { res??; }
            _ = sigterm.recv() => { info!("received SIGTERM"); }
            _ = sigint.recv()  => { info!("received SIGINT");  }
        }
    } else if let Some(socket) = cli.source_socket.clone() {
        info!(
            socket = %socket.display(),
            path   = %cli.source_path,
            tcp    = %addr,
            healthcheck = %cli.healthcheck,
            "source: unix socket",
        );
        let source = tokio::spawn(source_http::run_unix(
            socket, cli.source_path.clone(), tx, liveness, reconnect,
        ));
        tokio::select! {
            res = source       => { res??; }
            _ = sigterm.recv() => { info!("received SIGTERM"); }
            _ = sigint.recv()  => { info!("received SIGINT");  }
        }
    }

    info!("shutdown complete");
    Ok(())
}
