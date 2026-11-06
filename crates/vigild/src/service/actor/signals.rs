// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Signal helpers — stop/kill signal sending.

use std::time::Duration;

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use tracing::warn;

use super::dur;

use super::super::DEFAULT_KILL_DELAY;
use super::Actor;

impl Actor {
    // -----------------------------------------------------------------------
    // Signal helpers
    // -----------------------------------------------------------------------

    pub(super) fn stop_signal(&self) -> Signal {
        self.config.stop_signal.unwrap_or_default().0
    }

    pub(super) fn kill_delay(&self) -> Duration {
        dur(&self.config.kill_delay, DEFAULT_KILL_DELAY)
    }

    pub(super) fn send_stop_signal(&self) {
        self.send_signal(self.stop_signal());
    }

    pub(super) fn send_sigkill(&self) {
        self.send_signal(Signal::SIGKILL);
    }

    /// Send `signal` to the service's process group (the child leads it,
    /// see `process_group(0)`); if that fails, to the child alone.
    pub(super) fn send_signal(&self, signal: Signal) {
        let Some(pid) = self.child.as_ref().and_then(|c| c.id()) else {
            return;
        };
        let pid = pid as i32;
        if let Err(e) = kill(Pid::from_raw(-pid), signal) {
            // ESRCH: no such group (already gone, or the child is not a
            // group leader); the fallback below covers the latter.
            if e != nix::errno::Errno::ESRCH {
                warn!(service = %self.name, error = %e, ?signal, "signal to process group failed, sending to the process");
            }
            let _ = kill(Pid::from_raw(pid), signal);
        }
    }
}
