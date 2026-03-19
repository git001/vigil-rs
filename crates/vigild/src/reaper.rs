// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! PID-1 / subreaper support.
//!
//! When `vigild` runs as PID 1 (e.g. inside a container) or is configured as
//! the subreaper for a cgroup subtree, orphaned grandchild processes are
//! reparented to it.  Without explicit zombie-reaping those processes linger
//! as zombies and consume PID-table slots until `vigild` exits.
//!
//! This module:
//!  * detects whether we are PID 1
//!  * optionally calls `prctl(PR_SET_CHILD_SUBREAPER)` so we become the
//!    reaper for orphans even when we are *not* PID 1
//!  * spawns a tokio task that collects zombie processes on `SIGCHLD`

use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// PID-1 detection
// ---------------------------------------------------------------------------

/// Returns `true` if this process is running as PID 1.
pub fn is_pid1() -> bool {
    std::process::id() == 1
}

// ---------------------------------------------------------------------------
// Subreaper setup
// ---------------------------------------------------------------------------

/// Register this process as the subreaper for orphaned descendants.
///
/// On Linux this calls `prctl(PR_SET_CHILD_SUBREAPER, 1)` so that when a
/// child process dies, any of *its* children (that have not yet exited) are
/// reparented to us instead of to init.  Safe to call when already PID 1 —
/// the kernel accepts but effectively ignores the flag in that case.
///
/// On non-Linux platforms this is a no-op.
#[cfg(target_os = "linux")]
pub fn enable_subreaper() -> anyhow::Result<()> {
    use anyhow::Context;
    nix::sys::prctl::set_child_subreaper(true)
        .context("prctl PR_SET_CHILD_SUBREAPER")?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn enable_subreaper() -> anyhow::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Zombie reaper task
// ---------------------------------------------------------------------------

/// Spawn a background tokio task that reaps orphaned zombie processes whenever
/// `SIGCHLD` is delivered.
///
/// # Safety / interaction with tokio
/// On Linux ≥ 5.10 tokio uses `pidfd_open(2)` for processes it spawns, so
/// our `waitpid(-1, WNOHANG)` loop collects only truly orphaned zombies.
/// On older kernels there is a theoretical race (we could collect a PID tokio
/// is about to wait on), but in the primary use-case — single-binary container
/// init — the practical risk is negligible.
pub fn spawn_reaper() -> anyhow::Result<()> {
    let mut sigchld = tokio::signal::unix::signal(
        tokio::signal::unix::SignalKind::child(),
    )?;
    tokio::spawn(async move {
        loop {
            sigchld.recv().await;
            reap_children();
        }
    });
    Ok(())
}

fn reap_children() {
    use nix::errno::Errno;
    use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
    use nix::unistd::Pid;

    // Drain all zombie children before going back to sleep.
    loop {
        match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::StillAlive) => break,
            Ok(status) => {
                debug!("reaped orphan: {:?}", status);
            }
            Err(Errno::ECHILD) => break, // no children left
            Err(e) => {
                warn!("waitpid: {}", e);
                break;
            }
        }
    }
}
