// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The Unix signals vigild reacts to (binary only, not part of the library).
//!
//! SIGTERM / SIGINT / SIGQUIT  →  graceful shutdown
//! SIGHUP / SIGUSR1 / SIGUSR2  →  forward to all running service processes

use nix::sys::signal::Signal;
use tokio::signal::unix::{self, SignalKind};

/// A signal vigild received.
pub(crate) enum Received {
    /// Shut down.
    Stop(Signal),
    /// Forward to the services.
    Forward(Signal),
}

pub(crate) struct Signals {
    term: unix::Signal,
    int: unix::Signal,
    quit: unix::Signal,
    hup: unix::Signal,
    usr1: unix::Signal,
    usr2: unix::Signal,
}

impl Signals {
    pub(crate) fn new() -> std::io::Result<Self> {
        Ok(Self {
            term: unix::signal(SignalKind::terminate())?,
            int: unix::signal(SignalKind::interrupt())?,
            quit: unix::signal(SignalKind::quit())?,
            hup: unix::signal(SignalKind::hangup())?,
            usr1: unix::signal(SignalKind::user_defined1())?,
            usr2: unix::signal(SignalKind::user_defined2())?,
        })
    }

    /// The next signal.
    pub(crate) async fn recv(&mut self) -> Received {
        tokio::select! {
            _ = self.term.recv() => Received::Stop(Signal::SIGTERM),
            _ = self.int.recv()  => Received::Stop(Signal::SIGINT),
            _ = self.quit.recv() => Received::Stop(Signal::SIGQUIT),
            _ = self.hup.recv()  => Received::Forward(Signal::SIGHUP),
            _ = self.usr1.recv() => Received::Forward(Signal::SIGUSR1),
            _ = self.usr2.recv() => Received::Forward(Signal::SIGUSR2),
        }
    }
}
