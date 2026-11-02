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
//!  * keeps the exit statuses it collected, so that a waiter it beat
//!    ([`wait_child`]) still gets its child's

use std::collections::HashMap;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::process::Child;
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
    nix::sys::prctl::set_child_subreaper(true).context("prctl PR_SET_CHILD_SUBREAPER")?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn enable_subreaper() -> anyhow::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Zombie reaper task
// ---------------------------------------------------------------------------

/// Spawn a background tokio task that reaps zombie processes whenever
/// `SIGCHLD` is delivered.
///
/// # Interaction with tokio
/// `waitpid(-1, WNOHANG)` collects *any* exited child, including the ones
/// vigild spawned itself (services, exec checks) and waits on through tokio:
/// a pidfd tells tokio when its child has exited, but does not keep another
/// `waitpid` from collecting it first. When the reaper wins, tokio's wait
/// fails with `ECHILD` and the child's exit status would be lost (a service
/// that exited 0 was reported as failed, exit code 1), so the reaper keeps
/// what it collected for a while ([`REAPED_KEEP`]) and [`wait_child`] takes
/// it from there.
pub fn spawn_reaper() -> anyhow::Result<()> {
    let mut sigchld = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child())?;
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
                debug!("reaped: {:?}", status);
                record_reaped(status);
            }
            Err(Errno::ECHILD) => break, // no children left
            Err(e) => {
                warn!("waitpid: {}", e);
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Exit statuses the reaper collected
// ---------------------------------------------------------------------------

/// How long the reaper keeps a status it collected. A waiter whose wait it
/// beat asks within milliseconds; an orphan's status is never asked for, and
/// is dropped after this.
pub const REAPED_KEEP: Duration = Duration::from_secs(60);

/// How long [`wait_child`] looks for a status the reaper collected: the
/// reaper records it right after its `waitpid`, but a waiter can see `ECHILD`
/// in between.
const REAPED_LOOKUP: Duration = Duration::from_millis(200);

/// Raw wait statuses by PID, with when they were collected.
fn reaped() -> &'static Mutex<HashMap<i32, (Instant, i32)>> {
    static REAPED: OnceLock<Mutex<HashMap<i32, (Instant, i32)>>> = OnceLock::new();
    REAPED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Keeps a collected child's exit status, as the raw wait status
/// `ExitStatus::from_raw` takes, for [`wait_child`]. Statuses older than
/// [`REAPED_KEEP`] are dropped.
fn record_reaped(status: nix::sys::wait::WaitStatus) {
    use nix::sys::wait::WaitStatus;

    let (pid, raw) = match status {
        WaitStatus::Exited(pid, code) => (pid, (code & 0xff) << 8),
        WaitStatus::Signaled(pid, signal, core) => {
            (pid, signal as i32 | if core { 0x80 } else { 0 })
        }
        // Stopped / continued children are not collected (no WUNTRACED).
        _ => return,
    };
    let now = Instant::now();
    let mut map = reaped().lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|_, (at, _)| now.duration_since(*at) < REAPED_KEEP);
    map.insert(pid.as_raw(), (now, raw));
}

/// The exit status the reaper collected for `pid`, if it did, removed from
/// the record.
///
/// Only a status collected at or after `spawned_at` can belong to the child
/// we wait for. An older entry is the status of an earlier process that had
/// the same PID (e.g. an orphan reaped within [`REAPED_KEEP`] before the PID
/// was reused); it is discarded instead of being returned.
fn take_reaped(pid: u32, spawned_at: Instant) -> Option<ExitStatus> {
    let mut map = reaped().lock().unwrap_or_else(|e| e.into_inner());
    match map.get(&(pid as i32)) {
        Some((at, _)) if *at < spawned_at => {
            map.remove(&(pid as i32));
            None
        }
        Some(_) => map
            .remove(&(pid as i32))
            .map(|(_, raw)| ExitStatus::from_raw(raw)),
        None => None,
    }
}

/// Wait for `child` to exit, as `Child::wait` does, and get its exit status
/// even when the reaper collected it first.
///
/// Use this instead of `child.wait()` for every child vigild spawns while the
/// reaper runs (PID 1 or `--reaper`). Without the reaper it is
/// `child.wait()`.
///
/// `spawned_at` must be taken right before the child was spawned; it keeps a
/// stale status of an earlier process with the same PID from being returned.
pub async fn wait_child(child: &mut Child, spawned_at: Instant) -> std::io::Result<ExitStatus> {
    // Before the wait: tokio forgets the PID once the child is waited for.
    let pid = child.id();
    match child.wait().await {
        Err(e) if e.raw_os_error() == Some(nix::errno::Errno::ECHILD as i32) => {
            let Some(pid) = pid else { return Err(e) };
            let deadline = Instant::now() + REAPED_LOOKUP;
            loop {
                if let Some(status) = take_reaped(pid, spawned_at) {
                    return Ok(status);
                }
                if Instant::now() >= deadline {
                    return Err(e);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::wait::waitpid;
    use nix::unistd::Pid;

    /// Collects `child` the way the reaper does, before its waiter can.
    fn reap_first(child: &Child) {
        let pid = Pid::from_raw(child.id().expect("child has a pid") as i32);
        let status = waitpid(pid, None).expect("waitpid");
        record_reaped(status);
    }

    #[tokio::test]
    async fn exit_code_survives_the_reaper_collecting_it_first() {
        let spawned = Instant::now();
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", "exit 3"])
            .spawn()
            .unwrap();
        reap_first(&child);
        let status = wait_child(&mut child, spawned)
            .await
            .expect("status recovered");
        assert_eq!(status.code(), Some(3));
    }

    #[tokio::test]
    async fn success_survives_the_reaper_collecting_it_first() {
        let spawned = Instant::now();
        let mut child = tokio::process::Command::new("true").spawn().unwrap();
        reap_first(&child);
        let status = wait_child(&mut child, spawned)
            .await
            .expect("status recovered");
        assert!(status.success());
    }

    #[tokio::test]
    async fn signal_survives_the_reaper_collecting_it_first() {
        use std::os::unix::process::ExitStatusExt;
        let spawned = Instant::now();
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", "kill -TERM $$"])
            .spawn()
            .unwrap();
        reap_first(&child);
        let status = wait_child(&mut child, spawned)
            .await
            .expect("status recovered");
        assert_eq!(
            status.signal(),
            Some(nix::sys::signal::Signal::SIGTERM as i32)
        );
    }

    #[tokio::test]
    async fn without_the_reaper_it_is_child_wait() {
        let spawned = Instant::now();
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", "exit 7"])
            .spawn()
            .unwrap();
        let status = wait_child(&mut child, spawned).await.unwrap();
        assert_eq!(status.code(), Some(7));
    }

    /// S3 rest: a status recorded before the child was spawned belongs to an
    /// earlier process with the same PID and must not be returned.
    #[test]
    fn status_recorded_before_spawn_is_ignored() {
        let pid: u32 = 4_000_001; // beyond any real pid_max default
        let before = Instant::now();
        record_reaped(nix::sys::wait::WaitStatus::Exited(
            Pid::from_raw(pid as i32),
            7,
        ));
        let spawned_later = Instant::now() + Duration::from_millis(1);
        assert!(
            take_reaped(pid, spawned_later).is_none(),
            "stale status returned"
        );
        assert!(
            take_reaped(pid, before).is_none(),
            "stale entry must have been discarded"
        );

        let spawned = Instant::now();
        record_reaped(nix::sys::wait::WaitStatus::Exited(
            Pid::from_raw(pid as i32),
            3,
        ));
        let st = take_reaped(pid, spawned).expect("fresh status");
        assert_eq!(st.code(), Some(3));
    }

    #[test]
    fn statuses_older_than_the_keep_window_are_dropped() {
        let old = Instant::now() - REAPED_KEEP - Duration::from_secs(1);
        reaped().lock().unwrap().insert(-42, (old, 0));
        record_reaped(nix::sys::wait::WaitStatus::Exited(Pid::from_raw(-43), 0));
        let map = reaped().lock().unwrap();
        assert!(!map.contains_key(&-42));
        assert!(map.contains_key(&-43));
    }
}
