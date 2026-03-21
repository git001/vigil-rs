// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Signal helpers — stop/kill signal sending.

use std::time::Duration;

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use tracing::{debug, warn};

use crate::duration::parse_duration;

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
        self.config
            .kill_delay
            .as_deref()
            .and_then(|s| parse_duration(s).ok())
            .unwrap_or(DEFAULT_KILL_DELAY)
    }

    pub(super) fn send_stop_signal(&self) {
        if let Some(child) = &self.child
            && let Some(pid) = child.id()
        {
            let pgid = Pid::from_raw(-(pid as i32));
            let sig = self.stop_signal();
            if let Err(e) = kill(pgid, sig) {
                warn!(service = %self.name, error = %e, "kill(pgid) failed, retrying with pid");
                let _ = kill(Pid::from_raw(pid as i32), sig);
            }
        }
    }

    pub(super) fn send_signal(&self, signal: Signal) {
        if let Some(child) = &self.child
            && let Some(pid) = child.id()
        {
            let pgid = Pid::from_raw(-(pid as i32));
            if let Err(e) = kill(pgid, signal) {
                debug!(service = %self.name, error = %e, ?signal, "forward to pgid failed");
                let _ = kill(Pid::from_raw(pid as i32), signal);
            }
        }
    }

    pub(super) fn send_sigkill(&self) {
        if let Some(child) = &self.child
            && let Some(pid) = child.id()
        {
            let pgid = Pid::from_raw(-(pid as i32));
            if let Err(e) = kill(pgid, Signal::SIGKILL) {
                // ESRCH = process already exited — expected on clean shutdown
                if e != nix::errno::Errno::ESRCH {
                    warn!(service = %self.name, error = %e, "SIGKILL pgid failed");
                    let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
                }
            }
        }
    }
}
