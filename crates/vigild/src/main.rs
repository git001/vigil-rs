// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use vigild::{api, logs, overlord, path_prefix, reaper, server, tls};

use clap::Parser;
use nix::sys::signal::Signal;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use tokio::signal::unix::SignalKind;
use tracing::info;

/// vigild — Rust service supervisor daemon
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum TlsAnonymous {
    Open,
    Metrics,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ExitCode {
    Propagate,
    Fixed,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum LogFormat {
    Text,
    Json,
}

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
    "  POST   /v1/checks/refresh         run a check now\n",
    "  GET    /v1/health[?level=&names=] 200 when no check is down, else 502\n",
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

    /// Directory for everything vigild creates at runtime (control socket,
    /// sd_notify sockets). Checked at startup: must be a directory owned by
    /// vigild's UID, not a symlink, not writable by others. A missing last
    /// component is created (mode 0711). On OpenShift use e.g. /tmp/vigil.
    #[arg(long, env = "VIGIL_PATH_PREFIX", default_value = path_prefix::DEFAULT_PATH_PREFIX)]
    path_prefix: PathBuf,

    /// Unix socket path for the HTTP API. Default: <path-prefix>/vigild.sock.
    #[arg(long, env = "VIGIL_SOCKET")]
    socket: Option<PathBuf>,

    /// Permission bits of the Unix socket (octal). Default 600: owner only.
    /// Widen (e.g. 660 or 666) only together with local identities, because
    /// without identities every caller that can connect is admin.
    #[arg(long, env = "VIGIL_SOCKET_MODE", default_value = "600", value_parser = parse_octal_mode)]
    socket_mode: u32,

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

    /// Access of TLS callers without a matching identity: `open` (only
    /// /v1/checks, /v1/system-info, API docs) or `metrics` (also
    /// /v1/metrics). Logs, services and anything that changes state always
    /// need an identity over TLS.
    #[arg(long, env = "VIGIL_TLS_ANONYMOUS_ACCESS", value_enum, default_value_t = TlsAnonymous::Metrics)]
    tls_anonymous_access: TlsAnonymous,

    /// vigild's exit code when a policy shuts it down: `propagate` (the
    /// service's code after exit-code-map, 128+signal for signals) or `fixed`
    /// (v3 behaviour: 0 on success, 10 on failure).
    #[arg(long, env = "VIGIL_EXIT_CODE", value_enum, default_value_t = ExitCode::Propagate)]
    exit_code: ExitCode,

    /// Enable init/subreaper mode: reap orphaned zombie processes.
    /// Automatically active when vigild runs as PID 1.
    #[arg(long, env = "VIGIL_REAPER")]
    reaper: bool,

    /// Format of vigild's own log.
    #[arg(long, env = "VIGIL_LOG_FORMAT", value_enum, default_value_t = LogFormat::Text)]
    log_format: LogFormat,

    /// Reload the layers automatically when the files in --layers-dir
    /// change (e.g. a mounted ConfigMap), checking every INTERVAL
    /// (1s to 24h, e.g. 30s). Off by default.
    #[arg(long, env = "VIGIL_WATCH_LAYERS", value_name = "INTERVAL", value_parser = vigild::duration::parse_interval)]
    watch_layers: Option<std::time::Duration>,

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
        tracing_subscriber::EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into())
    };
    match args.log_format {
        LogFormat::Json => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(env_filter)
                .init();
        }
        _ => {
            tracing_subscriber::fmt().with_env_filter(env_filter).init();
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

    // Hide vigild's own environment (possibly holding secrets from the
    // container spec) from services running with the same UID: with
    // dumpable=0, /proc/<vigild>/environ and friends become root-owned.
    // Needs no capabilities; children are unaffected (reset on execve).
    #[cfg(target_os = "linux")]
    match nix::sys::prctl::set_dumpable(false) {
        Ok(()) => tracing::debug!("PR_SET_DUMPABLE=0"),
        Err(e) => tracing::warn!(%e, "prctl(PR_SET_DUMPABLE, 0) failed"),
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
        path_prefix = %args.path_prefix.display(),
        "vigild starting"
    );

    // Zombie reaper task (SIGCHLD → waitpid loop)
    if use_reaper {
        reaper::spawn_reaper()?;
    }

    // Check/create the runtime directory and bind the control socket before
    // any service starts: both are fatal on error and must not leave already
    // spawned services behind.
    path_prefix::prepare(&args.path_prefix)?;
    let socket = args
        .socket
        .clone()
        .unwrap_or_else(|| path_prefix::socket_path(&args.path_prefix));
    let unix_listener = server::bind_unix(&socket, args.socket_mode)?;
    // The TLS listener and certificate too: a busy address or a broken
    // --cert/--key must stop vigild before it starts any service.
    let tls = match args.tls_addr.as_deref() {
        Some(addr) => {
            let listener = server::bind_tls(addr)?;
            let (cert, key, client_ca) = (
                args.cert.clone(),
                args.key.clone(),
                args.tls_client_ca.clone(),
            );
            let hostname = server::tls_hostname(addr).to_owned();
            let acceptor = vigil_files::off_runtime("TLS files", move || {
                tls::load_or_generate(
                    cert.as_deref(),
                    key.as_deref(),
                    &hostname,
                    client_ca.as_deref(),
                )
            })
            .await?;
            Some((listener, acceptor))
        }
        None => None,
    };

    let http_address = socket.to_string_lossy().into_owned();
    // The bound address: with port 0 the kernel's choice, not ":0".
    let https_address = tls
        .as_ref()
        .map(|(l, _)| l.local_addr().map(|a| a.to_string()))
        .transpose()?;
    let overlord::Spawned {
        handle: overlord,
        log_store,
        metrics,
        identity_store,
        task: overlord_task,
    } = overlord::spawn(
        args.layers_dir,
        http_address,
        https_address,
        args.log_buffer,
        match args.exit_code {
            ExitCode::Fixed => vigild::service::ExitCodeMode::Fixed,
            ExitCode::Propagate => vigild::service::ExitCodeMode::Propagate,
        },
        path_prefix::notify_dir(&args.path_prefix),
        args.watch_layers,
    )
    .await?;

    let (shutdown_tx, mut shutdown_rx) =
        tokio::sync::mpsc::channel::<vigil_types::api::DaemonAction>(1);

    let tls_anonymous_access = match args.tls_anonymous_access {
        TlsAnonymous::Open => vigil_types::identity::IdentityAccess::Open,
        TlsAnonymous::Metrics => vigil_types::identity::IdentityAccess::Metrics,
    };
    if args.tls_addr.is_some() {
        info!(
            access = ?args.tls_anonymous_access,
            "TLS API: callers without a matching identity get this access level"
        );
    }

    let app_state = api::AppState {
        overlord: overlord.clone(),
        log_store,
        identity_store,
        metrics,
        shutdown_tx,
        tls_anonymous_access,
    };
    let router = api::router(app_state);

    // Unix socket server.
    let unix_router = router.clone();
    let unix_task = tokio::spawn(async move {
        if let Err(e) = server::serve_unix(unix_listener, unix_router).await {
            tracing::error!(%e, "Unix API server error");
        }
    });

    // Optional TLS server
    let tls_task = if let Some((listener, acceptor)) = tls {
        let tls_router = router.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = server::serve_tls(listener, acceptor, tls_router).await {
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

fn parse_octal_mode(s: &str) -> Result<u32, String> {
    let digits = s.strip_prefix("0o").unwrap_or(s);
    let mode =
        u32::from_str_radix(digits, 8).map_err(|e| format!("invalid octal mode {s:?}: {e}"))?;
    if mode > 0o777 {
        return Err(format!("mode {s:?} out of range (max 777)"));
    }
    Ok(mode)
}
