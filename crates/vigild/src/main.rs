// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use vigild::{api, overlord, path_prefix, reaper, server, tls};

use clap::Parser;
use std::os::unix::process::CommandExt;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tracing::info;
use vigil_types::api::DaemonAction;

mod cli;
mod signals;

use cli::{Args, LogFormat};
use signals::{Received, Signals};

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
        LogFormat::Text => {
            // Colours only on a terminal (not in `podman logs` or a file),
            // and not with NO_COLOR set.
            let colour = std::io::IsTerminal::is_terminal(&std::io::stdout())
                && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
            tracing_subscriber::fmt()
                .with_ansi(colour)
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
        .worker_threads(vigil_types::runtime::worker_threads(
            args.worker_threads,
            vigil_types::runtime::available_cpus(),
        ))
        .max_blocking_threads(vigil_types::runtime::MAX_BLOCKING_THREADS)
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
    let tls = bind_tls(&args).await?;

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
        args.layers_dir.clone(),
        http_address,
        https_address,
        vigild::logs::LogOptions {
            buffer: args.log_buffer,
            max_line: args.log_max_line_bytes,
        },
        args.exit_code_mode(),
        path_prefix::notify_dir(&args.path_prefix),
        args.watch_layers,
    )
    .await?;

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<DaemonAction>(1);

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
        tls_anonymous_access: args.anonymous_access(),
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
    let tls_task = tls.map(|(listener, acceptor)| {
        tokio::spawn(async move {
            if let Err(e) = server::serve_tls(listener, acceptor, router).await {
                tracing::error!(%e, "TLS API server error");
            }
        })
    });

    let mut signals = Signals::new()?;
    let restart = loop {
        tokio::select! {
            received = signals.recv() => match received {
                Received::Stop(signal) => {
                    info!("received {}", signal.as_str());
                    break false;
                }
                Received::Forward(signal) => {
                    info!("received {}, forwarding to services", signal.as_str());
                    overlord.tx.send(overlord::Cmd::ForwardSignal { signal }).await.ok();
                }
            },

            action = shutdown_rx.recv() => {
                match action {
                    Some(DaemonAction::Stop) => info!("daemon stop requested via API"),
                    Some(DaemonAction::Restart) => info!("daemon restart requested via API"),
                    None => {}
                }
                break action == Some(DaemonAction::Restart);
            }
        }
    };

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

/// Bind `--tls-addr` and load (or generate) the certificate, before any
/// service starts: a busy address or a broken `--cert`/`--key` must stop
/// vigild first. `None` without `--tls-addr`.
async fn bind_tls(args: &Args) -> anyhow::Result<Option<(TcpListener, TlsAcceptor)>> {
    let Some(addr) = args.tls_addr.as_deref() else {
        return Ok(None);
    };
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
    Ok(Some((listener, acceptor)))
}
