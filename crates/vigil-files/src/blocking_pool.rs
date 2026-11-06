// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Reading files without blocking the async runtime (feature `tokio`).
//!
//! A read on a dead NFS hard mount never returns and cannot be cancelled.
//! Reads at run time therefore run on tokio's blocking pool with
//! [`FILE_IO_TIMEOUT`]; a hanging read ties up one blocking thread, never
//! the runtime.

use std::time::Duration;

use tokio::task::JoinHandle;

/// Longest time to wait for file reads.
pub const FILE_IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Run `f` (which may read files) on the blocking pool, giving up after
/// [`FILE_IO_TIMEOUT`]. For one-off reads (a start, a replan). For reads
/// repeated in a loop use [`PeriodicRead`], which does not pile up hanging
/// reads.
pub async fn off_runtime<T: Send + 'static>(
    what: &str,
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    off_runtime_within(FILE_IO_TIMEOUT, what, f).await
}

/// [`off_runtime`] with another time limit.
pub async fn off_runtime_within<T: Send + 'static>(
    limit: Duration,
    what: &str,
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    match tokio::time::timeout(limit, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(res)) => res,
        Ok(Err(e)) => Err(anyhow::anyhow!("{what}: {e}")),
        Err(_) => Err(anyhow::anyhow!(
            "{what}: reading files did not finish within {:?} (hanging file system, e.g. a dead NFS mount?)",
            limit
        )),
    }
}

/// Result of a [`PeriodicRead`] round.
#[derive(Debug, PartialEq, Eq)]
pub enum ReadOutcome<T> {
    Done(T),
    /// The read of an earlier round still hangs; no new one was started.
    StillHanging,
    /// The read did not finish within the time limit (log it); it is kept,
    /// and later rounds return `StillHanging` until it returns.
    TimedOut,
    /// The read panicked.
    Failed,
}

/// A read repeated every round (`watch-files`, `--watch-layers`, a
/// credentials file per reconnect). While a read hangs no new one is
/// started, so a dead mount ties up at most one blocking thread per reader.
#[derive(Debug)]
pub struct PeriodicRead<T> {
    inflight: Option<JoinHandle<T>>,
}

impl<T> Default for PeriodicRead<T> {
    fn default() -> Self {
        Self { inflight: None }
    }
}

impl<T: Send + 'static> PeriodicRead<T> {
    pub async fn run(&mut self, f: impl FnOnce() -> T + Send + 'static) -> ReadOutcome<T> {
        self.run_within(FILE_IO_TIMEOUT, f).await
    }

    /// [`run`](Self::run) with another time limit.
    pub async fn run_within(
        &mut self,
        limit: Duration,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> ReadOutcome<T> {
        if self.is_hanging() {
            return ReadOutcome::StillHanging;
        }
        // A read that finished late is outdated; start a fresh one.
        self.inflight = None;
        let mut handle = tokio::task::spawn_blocking(f);
        match tokio::time::timeout(limit, &mut handle).await {
            Ok(Ok(v)) => ReadOutcome::Done(v),
            Ok(Err(_)) => ReadOutcome::Failed,
            Err(_) => {
                self.inflight = Some(handle);
                ReadOutcome::TimedOut
            }
        }
    }

    /// Whether a timed-out read has still not returned.
    pub fn is_hanging(&self) -> bool {
        self.inflight.as_ref().is_some_and(|h| !h.is_finished())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn off_runtime_gives_up_on_a_hanging_read() {
        let (_keep, rx) = std::sync::mpsc::channel::<()>();
        let started = std::time::Instant::now();
        let err = off_runtime_within(Duration::from_millis(100), "templates", move || {
            let _ = rx.recv_timeout(Duration::from_secs(5)); // "dead NFS"
            Ok(())
        })
        .await
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(err.to_string().contains("hanging file system"), "{err}");
        // the runtime is still usable
        assert_eq!(off_runtime("x", || Ok(42)).await.unwrap(), 42);
    }

    #[tokio::test]
    async fn periodic_read_does_not_pile_up_while_hanging() {
        let mut r = PeriodicRead::<u32>::default();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let out = r
            .run_within(Duration::from_millis(50), move || {
                let _ = release_rx.recv(); // "dead NFS" until released
                1
            })
            .await;
        assert_eq!(out, ReadOutcome::TimedOut);
        assert!(r.is_hanging(), "kept, not started again");
        assert_eq!(r.run(|| 2).await, ReadOutcome::StillHanging);
        release_tx.send(()).unwrap();
        while r.is_hanging() {
            std::thread::sleep(Duration::from_millis(5));
        }
        // The late result is dropped, a fresh read runs.
        assert_eq!(r.run(|| 2).await, ReadOutcome::Done(2));
        assert!(r.inflight.is_none());
    }

    #[tokio::test]
    async fn periodic_read_reports_a_panic() {
        let mut r = PeriodicRead::<u32>::default();
        assert_eq!(r.run(|| panic!("boom")).await, ReadOutcome::Failed);
    }
}
