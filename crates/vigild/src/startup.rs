// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The start-up steps of vigild before and after the runtime is built.

use tokio::net::{TcpListener, UnixListener};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tracing::info;
use vigild::{reaper, server, tls};

use crate::cli::{Args, LogFormat};

/// Set up logging as `--log-format` says; `RUST_LOG` overrides the level.
pub fn init_logging(format: LogFormat) {
    let env_filter = if std::env::var("RUST_LOG").is_ok() {
        tracing_subscriber::EnvFilter::from_default_env()
    } else {
        tracing_subscriber::EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into())
    };
    match format {
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
}

/// Reap zombies when vigild is PID 1 or `--reaper` is set; returns whether
/// the reaper is in use. Called *before* the tokio runtime is built, so
/// that any child process created during start-up is already covered.
pub fn enable_reaper(args: &Args) -> anyhow::Result<bool> {
    let use_reaper = args.reaper || reaper::is_pid1();
    if use_reaper {
        if reaper::is_pid1() {
            info!("running as PID 1 — zombie-reaper active");
        } else {
            reaper::enable_subreaper()?;
            info!("subreaper mode enabled");
        }
    }
    Ok(use_reaper)
}

/// Hide vigild's own environment (possibly holding secrets from the
/// container spec) from services running with the same UID: with
/// dumpable=0, /proc/<vigild>/environ and friends become root-owned.
/// Needs no capabilities; children are unaffected (reset on execve).
pub fn hide_environment() {
    #[cfg(target_os = "linux")]
    match nix::sys::prctl::set_dumpable(false) {
        Ok(()) => tracing::debug!("PR_SET_DUMPABLE=0"),
        Err(e) => tracing::warn!(%e, "prctl(PR_SET_DUMPABLE, 0) failed"),
    }
}

/// Bind `--tls-addr` and load (or generate) the certificate, before any
/// service starts: a busy address or a broken `--cert`/`--key` must stop
/// vigild first. `None` without `--tls-addr`.
pub async fn bind_tls(args: &Args) -> anyhow::Result<Option<(TcpListener, TlsAcceptor)>> {
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

/// The tasks of the API servers.
pub struct Servers {
    unix: JoinHandle<()>,
    tls: Option<JoinHandle<()>>,
}

impl Servers {
    /// Serve `router` on the Unix socket and, if bound, over TLS.
    pub fn spawn(
        unix_listener: UnixListener,
        tls: Option<(TcpListener, TlsAcceptor)>,
        router: axum::Router,
    ) -> Self {
        let unix_router = router.clone();
        let unix = tokio::spawn(async move {
            if let Err(e) = server::serve_unix(unix_listener, unix_router).await {
                tracing::error!(%e, "Unix API server error");
            }
        });
        let tls = tls.map(|(listener, acceptor)| {
            tokio::spawn(async move {
                if let Err(e) = server::serve_tls(listener, acceptor, router).await {
                    tracing::error!(%e, "TLS API server error");
                }
            })
        });
        Servers { unix, tls }
    }

    pub fn abort(&self) {
        self.unix.abort();
        if let Some(t) = &self.tls {
            t.abort();
        }
    }
}
