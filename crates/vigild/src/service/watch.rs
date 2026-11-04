// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `watch-files`: notice when files a service depends on change.
//!
//! Polling, not inotify: the kubelet updates Secret/ConfigMap volumes by
//! swapping the `..data` symlink atomically, which inotify on the file does
//! not reliably see; polling also works on NFS and overlayfs.
//!
//! A change is reported only once the new content has been stable for one
//! polling round (ESO and the kubelet update several keys one after
//! another), so one rotation causes one reaction.
//!
//! Opt-in: only services with a `watch-files` block are watched; without it
//! vigild does not touch any file after the start.
//!
//! Reads run on the blocking pool with [`FILE_IO_TIMEOUT`]. A read on a dead
//! NFS hard mount never returns; while it hangs no new read is started, so
//! at most one blocking thread per service is tied up and the actor keeps
//! working.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use vigil_types::plan::{FileChangeAction, WatchFiles};

use crate::duration::parse_duration;
use tokio::task::JoinHandle;
use tracing::warn;

use crate::template::{ContentHasher, FILE_IO_TIMEOUT, FileRead};

pub const DEFAULT_WATCH_INTERVAL: Duration = Duration::from_secs(30);
/// Shortest allowed interval: every round reads and hashes all watched
/// files, which must not become a load on the node or a file server (NFS).
pub const MIN_WATCH_INTERVAL: Duration = Duration::from_secs(1);
/// Longest allowed interval.
pub const MAX_WATCH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// State of one watched file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Missing,
    Content(u64),
    /// Could not be read (e.g. permission denied, or the read timed out);
    /// keeps the previous state, so a transient error does not trigger a
    /// reaction. As a baseline it means "unknown": the next readable state
    /// is taken over without a reaction.
    Unreadable,
}

impl From<Option<u64>> for State {
    fn from(h: Option<u64>) -> Self {
        h.map_or(State::Missing, State::Content)
    }
}

/// Current state of `path`, hashed the same way as at start.
fn probe(path: &Path) -> State {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return State::Missing,
        Err(_) => return State::Unreadable,
    };
    if !f.metadata().is_ok_and(|m| m.is_file()) {
        return State::Unreadable;
    }
    let mut h = ContentHasher::default();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => return State::Content(h.finish()),
            Ok(n) => h.update(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return State::Unreadable,
        }
    }
}

/// Watched files of one running service process.
#[derive(Debug)]
pub struct FileWatch {
    pub action: FileChangeAction,
    pub interval: Duration,
    service: String,
    paths: Vec<PathBuf>,
    baseline: Vec<State>,
    pending: Option<Vec<State>>,
    /// A read that has not returned yet (hanging file system).
    inflight: Option<JoinHandle<Vec<State>>>,
}

fn probe_all(paths: Vec<PathBuf>) -> JoinHandle<Vec<State>> {
    tokio::task::spawn_blocking(move || paths.iter().map(|p| probe(p)).collect())
}

impl FileWatch {
    /// Watch for a process just started. `reads` are the files read while
    /// rendering its templates and env files. `None` without a `watch-files`
    /// block, with `on-change: ignore`, or if there are no files.
    pub async fn new(
        service: &str,
        cfg: Option<&WatchFiles>,
        reads: Vec<FileRead>,
    ) -> Option<Self> {
        let cfg = cfg?;
        let action = cfg.on_change.unwrap_or_default();
        if action == FileChangeAction::Ignore {
            return None;
        }
        let interval = cfg
            .interval
            .as_deref()
            .and_then(|s| parse_duration(s).ok())
            .unwrap_or(DEFAULT_WATCH_INTERVAL)
            // The plan check rejects values outside the range; clamp anyway
            // so an unchecked config can never poll in a tight loop.
            .clamp(MIN_WATCH_INTERVAL, MAX_WATCH_INTERVAL);
        let mut watch = match &cfg.paths {
            // Explicit paths: only these; the application reads them itself,
            // so their state is taken now.
            Some(paths) => {
                let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
                let n = paths.len();
                Self::with(service, action, interval, paths, vec![State::Unreadable; n])
            }
            None => {
                let (paths, baseline) = reads
                    .into_iter()
                    .map(|r| (r.path, State::from(r.hash)))
                    .unzip();
                Self::with(service, action, interval, paths, baseline)
            }
        };
        if watch.paths.is_empty() {
            return None;
        }
        if cfg.paths.is_some()
            && let Some(states) = watch.read().await
        {
            watch.baseline = states;
        }
        Some(watch)
    }

    fn with(
        service: &str,
        action: FileChangeAction,
        interval: Duration,
        paths: Vec<PathBuf>,
        baseline: Vec<State>,
    ) -> Self {
        Self {
            action,
            interval,
            service: service.to_owned(),
            paths,
            baseline,
            pending: None,
            inflight: None,
        }
    }

    /// Current states, or `None` if the files could not be read in time.
    async fn read(&mut self) -> Option<Vec<State>> {
        let mut handle = match self.inflight.take() {
            Some(h) if !h.is_finished() => {
                // Still hanging: do not pile up more blocked threads.
                self.inflight = Some(h);
                return None;
            }
            _ => probe_all(self.paths.clone()),
        };
        match tokio::time::timeout(FILE_IO_TIMEOUT, &mut handle).await {
            Ok(res) => res.ok(),
            Err(_) => {
                warn!(
                    service = %self.service,
                    files = ?self.paths,
                    "watched files could not be read within {}s (hanging file system?); \
                     not checking them until the read returns",
                    FILE_IO_TIMEOUT.as_secs()
                );
                self.inflight = Some(handle);
                None
            }
        }
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Read all files (off the async runtime) and compare. Returns the
    /// changed paths once a change has been stable for one round.
    pub async fn poll(&mut self) -> Option<Vec<PathBuf>> {
        let current = self.read().await?;
        self.compare(current)
    }

    fn compare(&mut self, mut current: Vec<State>) -> Option<Vec<PathBuf>> {
        for (c, b) in current.iter_mut().zip(self.baseline.iter_mut()) {
            if *c == State::Unreadable {
                *c = *b;
            } else if *b == State::Unreadable {
                // Unknown at start: take it over, no reaction.
                *b = *c;
            }
        }
        if current == self.baseline {
            self.pending = None;
            return None;
        }
        if self.pending.as_ref() != Some(&current) {
            // Changed, or still changing: wait one more round.
            self.pending = Some(current);
            return None;
        }
        let changed = self
            .paths
            .iter()
            .zip(current.iter().zip(&self.baseline))
            .filter(|(_, (c, b))| c != b)
            .map(|(p, _)| p.clone())
            .collect();
        self.baseline = current;
        self.pending = None;
        Some(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::content_hash;

    fn read(path: &Path, content: Option<&str>) -> FileRead {
        FileRead {
            path: path.to_path_buf(),
            hash: content.map(|c| content_hash(c.as_bytes())),
        }
    }

    #[tokio::test]
    async fn interval_is_clamped() {
        let r = || {
            vec![FileRead {
                path: "/x".into(),
                hash: None,
            }]
        };
        let with = |i: &str| WatchFiles {
            interval: Some(i.into()),
            ..Default::default()
        };
        let w = FileWatch::new("svc", Some(&with("1ms")), r())
            .await
            .unwrap();
        assert_eq!(w.interval, MIN_WATCH_INTERVAL);
        let w = FileWatch::new("svc", Some(&with("1000h")), r())
            .await
            .unwrap();
        assert_eq!(w.interval, MAX_WATCH_INTERVAL);
        let w = FileWatch::new("svc", Some(&with("5m")), r()).await.unwrap();
        assert_eq!(w.interval, Duration::from_secs(300));
    }

    #[test]
    fn unknown_baseline_is_taken_over_without_a_reaction() {
        let mut w = FileWatch::with(
            "svc",
            FileChangeAction::Restart,
            DEFAULT_WATCH_INTERVAL,
            vec!["/a".into()],
            vec![State::Unreadable],
        );
        assert_eq!(w.compare(vec![State::Content(1)]), None);
        assert_eq!(w.compare(vec![State::Content(1)]), None);
        assert_eq!(w.compare(vec![State::Unreadable]), None);
        assert_eq!(w.compare(vec![State::Content(2)]), None);
        assert_eq!(w.compare(vec![State::Content(2)]), Some(vec!["/a".into()]));
    }

    /// A read that never returns (dead NFS) is given up after the timeout,
    /// and no second read is started while it hangs.
    #[tokio::test]
    async fn hanging_read_times_out_and_is_not_repeated() {
        let mut w = FileWatch::with(
            "svc",
            FileChangeAction::Restart,
            DEFAULT_WATCH_INTERVAL,
            vec!["/a".into()],
            vec![State::Content(1)],
        );
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        w.inflight = Some(tokio::task::spawn_blocking(move || {
            let _ = release_rx.recv(); // "hangs" until released
            vec![State::Content(2)]
        }));
        // still hanging: returns at once, keeps the handle
        assert_eq!(w.poll().await, None);
        assert!(w.inflight.is_some());
        release_tx.send(()).unwrap();
        // let the blocking task finish
        while !w.inflight.as_ref().unwrap().is_finished() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // finished: a fresh read of the real path (missing → change pending)
        assert_eq!(w.poll().await, None);
        assert!(w.inflight.is_none());
        assert_eq!(w.poll().await, Some(vec!["/a".into()]));
    }

    #[test]
    fn streamed_hash_matches_start_hash() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big");
        let content = "x".repeat(200 * 1024 + 7);
        std::fs::write(&p, &content).unwrap();
        assert_eq!(probe(&p), State::Content(content_hash(content.as_bytes())));
        assert_eq!(probe(&dir.path().join("nope")), State::Missing);
        assert_eq!(probe(dir.path()), State::Unreadable);
    }

    #[tokio::test]
    async fn change_is_reported_once_after_it_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, "1").unwrap();
        let mut w = FileWatch::new(
            "svc",
            Some(&defaults()),
            vec![read(&a, Some("1")), read(&b, None)],
        )
        .await
        .unwrap();
        assert_eq!(w.action, FileChangeAction::Restart);
        assert_eq!(w.interval, DEFAULT_WATCH_INTERVAL);

        assert_eq!(w.poll().await, None, "unchanged");
        std::fs::write(&a, "2").unwrap();
        assert_eq!(w.poll().await, None, "first round: wait");
        std::fs::write(&b, "new").unwrap(); // still changing
        assert_eq!(w.poll().await, None, "changed again: wait");
        assert_eq!(w.poll().await, Some(vec![a.clone(), b.clone()]), "stable");
        assert_eq!(w.poll().await, None, "reported only once");

        // deleted counts as a change
        std::fs::remove_file(&b).unwrap();
        assert_eq!(w.poll().await, None);
        assert_eq!(w.poll().await, Some(vec![b.clone()]));
    }

    #[tokio::test]
    async fn change_and_revert_within_a_round_is_no_change() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        std::fs::write(&a, "1").unwrap();
        let mut w = FileWatch::new("svc", Some(&defaults()), vec![read(&a, Some("1"))])
            .await
            .unwrap();
        std::fs::write(&a, "2").unwrap();
        assert_eq!(w.poll().await, None);
        std::fs::write(&a, "1").unwrap();
        assert_eq!(w.poll().await, None);
        assert_eq!(w.poll().await, None);
    }

    #[tokio::test]
    async fn kubelet_style_symlink_swap_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir(d.join("..v1")).unwrap();
        std::fs::write(d.join("..v1/pw"), "one").unwrap();
        std::os::unix::fs::symlink("..v1", d.join("..data")).unwrap();
        std::os::unix::fs::symlink("..data/pw", d.join("pw")).unwrap();
        let pw = d.join("pw");
        let mut w = FileWatch::new("svc", Some(&defaults()), vec![read(&pw, Some("one"))])
            .await
            .unwrap();

        std::fs::create_dir(d.join("..v2")).unwrap();
        std::fs::write(d.join("..v2/pw"), "two").unwrap();
        std::os::unix::fs::symlink("..v2", d.join("..data_tmp")).unwrap();
        std::fs::rename(d.join("..data_tmp"), d.join("..data")).unwrap();
        assert_eq!(w.poll().await, None);
        assert_eq!(w.poll().await, Some(vec![pw]));
    }

    #[tokio::test]
    async fn explicit_paths_replace_the_files_read_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("app.conf");
        let secret = dir.path().join("secret");
        std::fs::write(&conf, "a").unwrap();
        std::fs::write(&secret, "s").unwrap();
        let cfg = WatchFiles {
            on_change: Some(FileChangeAction::Signal(vigil_types::signal::StopSignal(
                nix::sys::signal::Signal::SIGHUP,
            ))),
            interval: Some("5s".into()),
            paths: Some(vec![conf.display().to_string()]),
        };
        let mut w = FileWatch::new("svc", Some(&cfg), vec![read(&secret, Some("s"))])
            .await
            .unwrap();
        assert_eq!(w.paths(), std::slice::from_ref(&conf));
        assert_eq!(w.interval, Duration::from_secs(5));

        std::fs::write(&secret, "changed").unwrap();
        assert_eq!(w.poll().await, None);
        assert_eq!(w.poll().await, None, "secret is not watched");
        std::fs::write(&conf, "b").unwrap();
        w.poll().await;
        assert_eq!(w.poll().await, Some(vec![conf]));
    }

    fn defaults() -> WatchFiles {
        WatchFiles::default()
    }

    #[tokio::test]
    async fn nothing_to_watch() {
        let r = vec![FileRead {
            path: "/x".into(),
            hash: None,
        }];
        // opt-in: no watch-files block, no watching
        assert!(FileWatch::new("svc", None, r.clone()).await.is_none());
        assert!(
            FileWatch::new("svc", Some(&defaults()), vec![])
                .await
                .is_none()
        );
        let ignore = WatchFiles {
            on_change: Some(FileChangeAction::Ignore),
            ..Default::default()
        };
        assert!(
            FileWatch::new("svc", Some(&ignore), r.clone())
                .await
                .is_none()
        );
        let empty = WatchFiles {
            paths: Some(vec![]),
            ..Default::default()
        };
        assert!(FileWatch::new("svc", Some(&empty), r).await.is_none());
    }
}
