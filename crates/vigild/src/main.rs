// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use vigild::{api, overlord, path_prefix, reaper, server};

use clap::Parser;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tracing::info;
use vigil_types::api::DaemonAction;

mod cli;
mod shutdown;
mod signals;
mod startup;

use cli::Args;
use signals::Signals;
use startup::Servers;

fn main() -> anyhow::Result<()> {
    vigild::install_crypto_provider();

    let args = Args::parse();
    startup::init_logging(args.log_format);
    let use_reaper = startup::enable_reaper(&args)?;
    startup::hide_environment();

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
    let tls = startup::bind_tls(&args).await?;

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

    let stopping = Arc::new(AtomicBool::new(false));
    let app_state = api::AppState {
        overlord: overlord.clone(),
        log_store,
        identity_store,
        metrics,
        shutdown_tx,
        tls_anonymous_access: args.anonymous_access(),
        stopping: Arc::clone(&stopping),
    };
    let servers = Servers::spawn(unix_listener, tls, api::router(app_state));

    let mut signals = Signals::new()?;
    let stop = shutdown::wait_for_stop(&mut signals, &mut shutdown_rx, &overlord).await;
    if stop.by_signal && !args.shutdown_delay.is_zero() {
        shutdown::shutdown_delay(args.shutdown_delay, &stopping, &mut signals, &overlord).await;
    }

    info!("shutting down…");
    // Give any in-flight HTTP responses (e.g. the stop/restart API call itself) a moment to flush.
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    overlord.tx.send(overlord::Cmd::Shutdown).await.ok();
    servers.abort();

    // Wait for the overlord to finish stop_all() — it handles its own kill-delay timeouts.
    let _ = overlord_task.await;

    if stop.restart {
        shutdown::re_exec();
    }

    info!("vigild exited");
    Ok(())
}
