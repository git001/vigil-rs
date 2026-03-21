//! vigil-log-relay — read ndjson log streams and forward them to a TCP sink.
//!
//! Source modes (mutually exclusive):
//!
//!   --kubernetes
//!       Watch Running pods in a Kubernetes namespace via the K8s API.
//!
//!   --source-socket PATH  [--source-path /v1/logs/follow?format=ndjson]
//!       Read stream from a Unix-domain socket (e.g. vigild local socket).
//!
//!   --source-url URL
//!       Read stream from an HTTP/HTTPS URL (e.g. vigild TLS API).
//!
//! Output: newline-delimited JSON (ndjson) to a TCP listener.

use std::sync::Arc;

use anyhow::{Result, bail};
use clap::Parser;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tracing::info;

mod cli;
mod filter;
mod healthcheck;
mod source_http;
mod source_k8s;
mod source_unix;
mod source_url;
mod tcp_sink;

pub use cli::Cli;
pub use filter::LineFilter;
pub use healthcheck::Liveness;
pub use source_http::{ReconnectConfig, SourceConnConfig};
pub use tcp_sink::SinkConfig;

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let cli = Cli::parse();

    let level = if cli.debug {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };
    match cli.log_format.as_str() {
        "json" => {
            tracing_subscriber::fmt()
                .json()
                .with_max_level(level)
                .init();
        }
        _ => {
            if cli.debug {
                tracing_subscriber::fmt()
                    .with_target(true)
                    .with_timer(tracing_subscriber::fmt::time::SystemTime)
                    .with_max_level(level)
                    .init();
            } else {
                tracing_subscriber::fmt()
                    .with_target(false)
                    .without_time()
                    .with_max_level(level)
                    .init();
            }
        }
    }

    let source_count = usize::from(cli.kubernetes)
        + usize::from(cli.source_url.is_some())
        + usize::from(cli.source_socket.is_some());
    if source_count == 0 {
        bail!("specify exactly one source: --kubernetes, --source-url, or --source-socket");
    }

    let addr = format!("{}:{}", cli.tcp_sink_host, cli.tcp_sink_port);
    let (tx, rx) = mpsc::channel::<String>(8192);

    // TCP sink — single task owning the connection
    let sink_cfg = SinkConfig {
        connect_timeout_ms: cli.dest_connect_timeout,
        write_timeout_ms: cli.dest_read_timeout,
        idle_timeout_ms: cli.dest_idle_timeout,
        keepalive_interval_secs: cli.dest_keepalive_interval,
        keepalive_timeout_secs: cli.dest_keepalive_timeout,
        reconnect_delay_ms: cli.dest_reconnect_delay,
        reconnect_max_ms: cli.dest_reconnect_max,
    };
    tokio::spawn(tcp_sink::run(addr.clone(), rx, sink_cfg));

    // Healthcheck HTTP server
    let liveness = Liveness::new(cli.healthcheck_max_age);
    tokio::spawn(healthcheck::serve(
        cli.healthcheck.clone(),
        liveness.clone(),
    ));

    // Connection + reconnect config for HTTP source modes
    let source_conn = SourceConnConfig {
        connect_timeout_ms: cli.source_connect_timeout,
        read_timeout_ms: cli.source_read_timeout,
        idle_timeout_ms: cli.source_idle_timeout,
        keepalive_interval_secs: cli.source_keepalive_interval,
        keepalive_timeout_secs: cli.source_keepalive_timeout,
        source_insecure: cli.source_insecure,
        source_cacert: cli.source_cacert.clone(),
        proxy_url: cli.source_proxy.clone(),
        proxy_insecure: cli.source_proxy_insecure,
        proxy_cacert: cli.source_proxy_cacert.clone(),
        no_proxy: cli.source_no_proxy.clone(),
    };
    let reconnect = ReconnectConfig {
        initial_delay_ms: cli.source_reconnect_delay,
        max_delay_ms: cli.source_reconnect_max,
        max_retries: cli.source_reconnect_retries,
    };

    let filter = LineFilter::new(&cli.include, &cli.exclude)?;

    info!(version = env!("CARGO_PKG_VERSION"), "vigil-log-relay starting");

    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    if cli.kubernetes {
        info!(
            namespace   = %cli.namespace,
            selector    = cli.pod_selector.as_str(),
            tcp         = %addr,
            interval_s  = cli.watch_interval,
            healthcheck = %cli.healthcheck,
            "source: kubernetes pod logs",
        );
        let source = tokio::spawn(source_k8s::run(cli, tx, Arc::clone(&liveness), filter));
        tokio::select! {
            res = source        => { res??; }
            _ = sigterm.recv()  => { info!("received SIGTERM"); }
            _ = sigint.recv()   => { info!("received SIGINT");  }
        }
    } else if let Some(url) = cli.source_url.clone() {
        info!(url = %url, tcp = %addr, healthcheck = %cli.healthcheck, "source: http url");
        let source = tokio::spawn(source_url::run(
            url,
            tx,
            Arc::clone(&liveness),
            source_conn,
            reconnect,
            filter,
        ));
        tokio::select! {
            res = source        => { res??; }
            _ = sigterm.recv()  => { info!("received SIGTERM"); }
            _ = sigint.recv()   => { info!("received SIGINT");  }
        }
    } else if let Some(socket) = cli.source_socket.clone() {
        info!(
            socket      = %socket.display(),
            path        = %cli.source_path,
            tcp         = %addr,
            healthcheck = %cli.healthcheck,
            "source: unix socket",
        );
        let source = tokio::spawn(source_unix::run(
            socket,
            cli.source_path.clone(),
            tx,
            Arc::clone(&liveness),
            source_conn,
            reconnect,
            filter,
        ));
        tokio::select! {
            res = source        => { res??; }
            _ = sigterm.recv()  => { info!("received SIGTERM"); }
            _ = sigint.recv()   => { info!("received SIGINT");  }
        }
    }

    info!("shutdown complete");
    Ok(())
}
