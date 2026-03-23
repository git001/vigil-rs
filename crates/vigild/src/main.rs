// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use vigild::{api, identity, logs, overlord, reaper, server, tls};

use clap::Parser;
use nix::sys::signal::Signal;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use tokio::signal::unix::SignalKind;
use tracing::info;

/// vigild — Rust service supervisor daemon
#[derive(Debug, Parser)]
#[command(name = "vigild", version, about, long_about = concat!(
    "vigild — Rust service supervisor daemon\n",
    "\n",
    "Reads layer YAML files from --layers-dir, merges them into a plan, and\n",
    "supervises the declared services and health checks.\n",
    "\n",
    "HTTP API (Unix socket, default /run/vigil/vigild.sock)\n",
    "  GET    /v1/system-info            daemon version, boot-id, start-time\n",
    "  GET    /v1/services[?names=]      list services and their status\n",
    "  POST   /v1/services               start / stop / restart services\n",
    "  GET    /v1/changes/{id}           inspect a change record\n",
    "  GET    /v1/checks[?names=]        list health checks\n",
    "  GET    /v1/alerts[?names=]        list alert configurations and check status\n",
    "  GET    /v1/logs[?services=&n=]    tail service stdout/stderr\n",
    "  GET    /v1/logs/follow[?services=] stream logs as SSE\n",
    "  POST   /v1/replan                 reload layers from disk\n",
    "  GET    /v1/metrics                Prometheus/OpenMetrics exposition\n",
    "  POST   /v1/vigild                 stop or restart the daemon\n",
    "  GET    /v1/identities[?names=]    list identities\n",
    "  POST   /v1/identities             add or update identities\n",
    "  DELETE /v1/identities             remove identities\n",
    "  GET    /docs                      Swagger UI\n",
    "  GET    /openapi.json              OpenAPI 3.0 spec\n",
    "\n",
    "Use 'vigil --help' for the command-line client.",
))]
struct Args {
    /// Directory containing layer YAML files.
    #[arg(long, env = "VIGIL_LAYERS", default_value = "/etc/vigil/layers")]
    layers_dir: PathBuf,

    /// Unix socket path for the HTTP API.
    #[arg(long, env = "VIGIL_SOCKET", default_value = "/run/vigil/vigild.sock")]
    socket: PathBuf,

    /// Address for the HTTPS API (e.g. "0.0.0.0:8443"). Disabled if not set.
    #[arg(long, env = "VIGIL_TLS_ADDR")]
    tls_addr: Option<String>,

    /// PEM certificate file for TLS. Auto-generates self-signed if omitted.
    #[arg(long, env = "VIGIL_CERT")]
    cert: Option<PathBuf>,

    /// PEM private key file for TLS. Auto-generates self-signed if omitted.
    #[arg(long, env = "VIGIL_KEY")]
    key: Option<PathBuf>,

    /// PEM file with one or more CA certificates used to verify TLS client
    /// certificates (mTLS).  Supports chain files with multiple concatenated
    /// PEM blocks.  Client certificates are **optional** — connections without
    /// a client cert still work and fall back to Basic Auth or local UID auth.
    /// Has no effect unless `--tls-addr` is also set.
    #[arg(long, env = "VIGIL_TLS_CLIENT_CA")]
    tls_client_ca: Option<PathBuf>,

    /// Enable init/subreaper mode: reap orphaned zombie processes.
    /// Automatically active when vigild runs as PID 1.
    #[arg(long, env = "VIGIL_REAPER")]
    reaper: bool,

    /// Log format: "text" or "json".
    #[arg(long, env = "VIGIL_LOG_FORMAT", default_value = "text")]
    log_format: String,

    /// Per-service log ring-buffer size (number of lines kept in memory).
    /// Also controls the SSE broadcast channel depth (half this value,
    /// clamped to [64, 4096]), which determines how many entries can queue
    /// for a slow log-stream consumer before it starts skipping.
    #[arg(long, env = "VIGIL_LOG_BUFFER", default_value_t = logs::DEFAULT_BUFFER_CAPACITY)]
    log_buffer: usize,
}

fn main() -> anyhow::Result<()> {
    vigild::install_crypto_provider();

    let args = Args::parse();

    let env_filter = if std::env::var("RUST_LOG").is_ok() {
        tracing_subscriber::EnvFilter::from_default_env()
    } else {
        tracing_subscriber::EnvFilter::from_default_env()
            .add_directive(tracing::Level::INFO.into())
    };
    match args.log_format.as_str() {
        "json" => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(env_filter)
                .init();
        }
        _ => {
            tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .init();
        }
    }

    // Enable subreaper *before* spawning the tokio runtime so that any child
    // processes created during startup are already covered.
    let use_reaper = args.reaper || reaper::is_pid1();
    if use_reaper {
        if reaper::is_pid1() {
            info!("running as PID 1 — zombie-reaper active");
        } else {
            reaper::enable_subreaper()?;
            info!("subreaper mode enabled");
        }
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(args, use_reaper))
}

async fn async_main(args: Args, use_reaper: bool) -> anyhow::Result<()> {
    info!(
        version = env!("CARGO_PKG_VERSION"),
        layers_dir = %args.layers_dir.display(),
        socket = %args.socket.display(),
        "vigild starting"
    );

    // Zombie reaper task (SIGCHLD → waitpid loop)
    if use_reaper {
        reaper::spawn_reaper()?;
    }

    let http_address = args.socket.to_string_lossy().into_owned();
    let https_address = args.tls_addr.clone();
    let (overlord, log_store, metrics, overlord_task) = overlord::spawn(
        args.layers_dir,
        http_address,
        https_address,
        args.log_buffer,
    )?;
    let identity_store = identity::IdentityStore::new();

    let (shutdown_tx, mut shutdown_rx) =
        tokio::sync::mpsc::channel::<vigil_types::api::DaemonAction>(1);

    let app_state = api::AppState {
        overlord: overlord.clone(),
        log_store,
        identity_store,
        metrics,
        shutdown_tx,
    };
    let router = api::router(app_state);

    // Unix socket server
    let socket = args.socket.clone();
    let unix_router = router.clone();
    let unix_task = tokio::spawn(async move {
        if let Err(e) = server::serve_unix(&socket, unix_router).await {
            tracing::error!(%e, "Unix API server error");
        }
    });

    // Optional TLS server
    let tls_task = if let Some(addr) = args.tls_addr {
        let hostname = addr.split(':').next().unwrap_or("localhost").to_string();
        let acceptor = tls::load_or_generate(
            args.cert.as_deref(),
            args.key.as_deref(),
            &hostname,
            args.tls_client_ca.as_deref(),
        )?;
        let tls_router = router.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = server::serve_tls(&addr, acceptor, tls_router).await {
                tracing::error!(%e, "TLS API server error");
            }
        });
        Some(handle)
    } else {
        None
    };

    // -----------------------------------------------------------------------
    // Signal handling loop
    //
    // SIGTERM / SIGINT / SIGQUIT  →  graceful shutdown
    // SIGHUP / SIGUSR1 / SIGUSR2  →  forward to all running service processes
    // -----------------------------------------------------------------------
    let mut sigterm = tokio::signal::unix::signal(SignalKind::terminate())?;
    let mut sigint = tokio::signal::unix::signal(SignalKind::interrupt())?;
    let mut sigquit = tokio::signal::unix::signal(SignalKind::quit())?;
    let mut sighup = tokio::signal::unix::signal(SignalKind::hangup())?;
    let mut sigusr1 = tokio::signal::unix::signal(SignalKind::user_defined1())?;
    let mut sigusr2 = tokio::signal::unix::signal(SignalKind::user_defined2())?;

    let mut restart = false;
    loop {
        tokio::select! {
            _ = sigterm.recv() => { info!("received SIGTERM"); break; }
            _ = sigint.recv()  => { info!("received SIGINT");  break; }
            _ = sigquit.recv() => { info!("received SIGQUIT"); break; }

            _ = sighup.recv() => {
                info!("received SIGHUP, forwarding to services");
                overlord.tx.send(overlord::Cmd::ForwardSignal { signal: Signal::SIGHUP }).await.ok();
            }
            _ = sigusr1.recv() => {
                info!("received SIGUSR1, forwarding to services");
                overlord.tx.send(overlord::Cmd::ForwardSignal { signal: Signal::SIGUSR1 }).await.ok();
            }
            _ = sigusr2.recv() => {
                info!("received SIGUSR2, forwarding to services");
                overlord.tx.send(overlord::Cmd::ForwardSignal { signal: Signal::SIGUSR2 }).await.ok();
            }

            action = shutdown_rx.recv() => {
                match action {
                    Some(vigil_types::api::DaemonAction::Stop) => {
                        info!("daemon stop requested via API");
                    }
                    Some(vigil_types::api::DaemonAction::Restart) => {
                        info!("daemon restart requested via API");
                        restart = true;
                    }
                    None => {}
                }
                break;
            }
        }
    }

    info!("shutting down…");
    // Give any in-flight HTTP responses (e.g. the stop/restart API call itself) a moment to flush.
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    overlord.tx.send(overlord::Cmd::Shutdown).await.ok();
    unix_task.abort();
    if let Some(t) = tls_task {
        t.abort();
    }

    // Wait for the overlord to finish stop_all() — it handles its own kill-delay timeouts.
    let _ = overlord_task.await;

    if restart {
        info!("re-executing vigild…");
        let args: Vec<String> = std::env::args().collect();
        let err = std::process::Command::new(&args[0]).args(&args[1..]).exec();
        tracing::error!(%err, "re-exec failed");
        std::process::exit(1);
    }

    info!("vigild exited");
    Ok(())
}
