// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! How vigild comes to a stop: a signal or the API, `--shutdown-delay`,
//! and the re-exec for a restart.

use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc;
use tracing::info;
use vigil_types::api::DaemonAction;
use vigild::overlord;

use crate::signals::{Received, Signals};

/// Why vigild stops.
pub struct Stop {
    /// `POST /v1/vigild` asked for a restart.
    pub restart: bool,
    /// A stop signal (not the API).
    pub by_signal: bool,
}

/// Forward signals to the services until a stop signal or the API ends
/// vigild.
pub async fn wait_for_stop(
    signals: &mut Signals,
    api: &mut mpsc::Receiver<DaemonAction>,
    overlord: &overlord::Handle,
) -> Stop {
    loop {
        tokio::select! {
            received = signals.recv() => match received {
                Received::Stop(signal) => {
                    info!("received {}", signal.as_str());
                    return Stop { restart: false, by_signal: true };
                }
                Received::Forward(signal) => forward(signal, overlord).await,
            },

            action = api.recv() => {
                match action {
                    Some(DaemonAction::Stop) => info!("daemon stop requested via API"),
                    Some(DaemonAction::Restart) => info!("daemon restart requested via API"),
                    None => {}
                }
                return Stop { restart: action == Some(DaemonAction::Restart), by_signal: false };
            }
        }
    }
}

/// `--shutdown-delay`: report not ready on /v1/health and keep the
/// services running for `delay`, so a load balancer stops sending requests
/// first. Signals to forward still go to the services; a second stop signal
/// ends the wait.
pub async fn shutdown_delay(
    delay: std::time::Duration,
    stopping: &AtomicBool,
    signals: &mut Signals,
    overlord: &overlord::Handle,
) {
    stopping.store(true, Ordering::Relaxed);
    info!(delay = ?delay, "shutdown delay: reporting not ready, services keep running");
    let deadline = tokio::time::Instant::now() + delay;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => return,
            received = signals.recv() => match received {
                Received::Stop(signal) => {
                    info!("received {} again, stopping now", signal.as_str());
                    return;
                }
                Received::Forward(signal) => forward(signal, overlord).await,
            },
        }
    }
}

async fn forward(signal: nix::sys::signal::Signal, overlord: &overlord::Handle) {
    info!("received {}, forwarding to services", signal.as_str());
    overlord
        .tx
        .send(overlord::Cmd::ForwardSignal { signal })
        .await
        .ok();
}

/// Start vigild anew with the same arguments; returns only on failure,
/// by exiting.
pub fn re_exec() -> ! {
    info!("re-executing vigild…");
    let args: Vec<String> = std::env::args().collect();
    let err = std::process::Command::new(&args[0]).args(&args[1..]).exec();
    tracing::error!(%err, "re-exec failed");
    std::process::exit(1);
}
