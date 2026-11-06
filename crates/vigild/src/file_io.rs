// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Reading files without blocking vigild: templates, env files, watched
//! files (`watch-files`) and layer files (`--watch-layers`, replan).
//!
//! A read on a dead NFS hard mount never returns and cannot be cancelled.
//! All reads therefore run on the blocking pool with [`FILE_IO_TIMEOUT`]; a
//! hanging read ties up one blocking thread, never the async runtime.

use std::hash::Hasher;
use std::time::Duration;

use tokio::task::JoinHandle;

/// Longest time vigild waits for file reads.
pub const FILE_IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Run `f` (which may read files) on the blocking pool, giving up after
/// [`FILE_IO_TIMEOUT`]. For one-off reads (a start, a replan).
pub async fn off_runtime<T: Send + 'static>(
    what: &str,
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    off_runtime_within(FILE_IO_TIMEOUT, what, f).await
}

pub(crate) async fn off_runtime_within<T: Send + 'static>(
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
    /// The read did not finish within [`FILE_IO_TIMEOUT`] (log it); it is
    /// kept, and later rounds return `StillHanging` until it returns.
    TimedOut,
    /// The read panicked.
    Failed,
}

/// A read repeated every polling round (`watch-files`, `--watch-layers`).
/// While a read hangs no new one is started, so a dead mount ties up at
/// most one blocking thread per watcher.
#[derive(Debug)]
pub struct PeriodicRead<T> {
    pub(crate) inflight: Option<JoinHandle<T>>,
}

impl<T> Default for PeriodicRead<T> {
    fn default() -> Self {
        Self { inflight: None }
    }
}

impl<T: Send + 'static> PeriodicRead<T> {
    pub async fn run(&mut self, f: impl FnOnce() -> T + Send + 'static) -> ReadOutcome<T> {
        if self.inflight.as_ref().is_some_and(|h| !h.is_finished()) {
            return ReadOutcome::StillHanging;
        }
        // A read that finished late is outdated; start a fresh one.
        self.inflight = None;
        let mut handle = tokio::task::spawn_blocking(f);
        match tokio::time::timeout(FILE_IO_TIMEOUT, &mut handle).await {
            Ok(Ok(v)) => ReadOutcome::Done(v),
            Ok(Err(_)) => ReadOutcome::Failed,
            Err(_) => {
                self.inflight = Some(handle);
                ReadOutcome::TimedOut
            }
        }
    }
}

/// Hash used to detect content changes (not cryptographic; only compared
/// with earlier hashes within one vigild process). Feeding the same bytes in
/// any chunks gives the same hash, so a streamed file matches
/// [`content_hash`] of its content.
#[derive(Default)]
pub struct ContentHasher(std::collections::hash_map::DefaultHasher);

impl ContentHasher {
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    pub fn finish(&self) -> u64 {
        self.0.finish()
    }
}

impl std::io::Write for ContentHasher {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut h = ContentHasher::default();
    h.update(bytes);
    h.finish()
}

/// Feed everything `reader` yields into `h` (`Interrupted` is retried).
pub fn hash_into(h: &mut ContentHasher, reader: &mut impl std::io::Read) -> std::io::Result<()> {
    std::io::copy(reader, h).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn off_runtime_gives_up_on_a_hanging_read() {
        let (_keep, rx) = std::sync::mpsc::channel::<()>();
        let started = std::time::Instant::now();
        let err = off_runtime_within(
            std::time::Duration::from_millis(100),
            "templates",
            move || {
                let _ = rx.recv_timeout(std::time::Duration::from_secs(5)); // "dead NFS"
                Ok(())
            },
        )
        .await
        .unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(err.to_string().contains("hanging file system"), "{err}");
        // the runtime is still usable
        assert_eq!(off_runtime("x", || Ok(42)).await.unwrap(), 42);
    }

    #[tokio::test]
    async fn periodic_read_does_not_pile_up_while_hanging() {
        let mut r = PeriodicRead::<u32>::default();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        r.inflight = Some(tokio::task::spawn_blocking(move || {
            let _ = release_rx.recv(); // "hangs" until released
            1
        }));
        assert_eq!(r.run(|| 2).await, ReadOutcome::StillHanging);
        release_tx.send(()).unwrap();
        while !r.inflight.as_ref().unwrap().is_finished() {
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

    #[test]
    fn chunked_hash_equals_whole_hash() {
        let data = vec![7u8; 100_000];
        let mut h = ContentHasher::default();
        hash_into(&mut h, &mut &data[..]).unwrap();
        assert_eq!(h.finish(), content_hash(&data));
    }
}
