// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `--watch-layers`: replan automatically when the layer files change, e.g.
//! after a ConfigMap update.
//!
//! Polling, not inotify, for the same reasons as `watch-files`: the kubelet
//! swaps the ConfigMap's `..data` symlink atomically, and polling also works
//! on NFS and overlayfs. One hash covers the names and contents of all layer
//! files ([`is_layer_file`]; the kubelet's own `..` entries are skipped) and
//! the contents of the files the layers refer to ([`WatchedFiles`]: `tls:`
//! files, `password-hash-file`), so a rotated certificate or Kubernetes
//! Secret is picked up too.
//!
//! A change triggers a replan once it has been stable for one polling round,
//! so a ConfigMap with several files is applied once. The replan itself
//! validates first: invalid layers keep the running plan and send the alert
//! `vigild`/`layers`/`invalid`. A failed replan is not retried; only the
//! next change of the files triggers a new one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use super::Cmd;
use super::plan::is_layer_file;
use crate::duration::clamp_interval;
use vigil_files::{ContentHasher, FILE_IO_TIMEOUT, PeriodicRead, ReadOutcome, hash_into};

/// Files the layers refer to whose content matters as much as the layers'
/// own (`tls:` files, `password-hash-file`). The overlord replaces the list
/// after every successful load.
#[derive(Clone, Default)]
pub struct WatchedFiles(Arc<Mutex<Vec<PathBuf>>>);

impl WatchedFiles {
    pub fn new(files: Vec<PathBuf>) -> Self {
        Self(Arc::new(Mutex::new(files)))
    }

    pub fn set(&self, files: Vec<PathBuf>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = files;
    }

    fn get(&self) -> Vec<PathBuf> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Hash over the names and contents of the layer files in `dir`, in the
/// order `load` reads them, and over the contents of `secrets`. A missing
/// directory or secret file has its own hash (the next load reports it).
/// `None`: the directory or a layer file could not be read; the round is
/// skipped.
pub(super) fn layers_hash(dir: &Path, secrets: &[PathBuf]) -> Option<u64> {
    let mut h = ContentHasher::default();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            h.update(b"\0missing");
            return Some(h.finish());
        }
        Err(_) => return None,
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| is_layer_file(n))
        .collect();
    names.sort();
    for name in names {
        h.update(name.as_bytes());
        h.update(b"\0");
        let mut f = std::fs::File::open(dir.join(&name)).ok()?;
        hash_into(&mut h, &mut f).ok()?;
        // Separates this file's content from the next file's name.
        h.update(b"\0");
    }
    for path in secrets {
        h.update(path.as_os_str().as_encoded_bytes());
        h.update(b"\0");
        match std::fs::File::open(path) {
            Ok(mut f) => hash_into(&mut h, &mut f).ok()?,
            Err(_) => h.update(b"\0missing"),
        }
        h.update(b"\0");
    }
    Some(h.finish())
}

/// The state of the layers the running plan was loaded from.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    /// Not known (the files could not be read): the first poll sets it.
    #[default]
    Unknown,
    /// The hash of the files as they were loaded.
    Known(u64),
    /// The layer files changed while they were loaded: the plan may be
    /// older than the files, the first stable poll replans.
    Stale,
}

impl Baseline {
    /// The baseline of a load: `before` and `after` hash the layer files
    /// alone, right before and after it; `full` hashes them after it
    /// together with the files the loaded layers refer to.
    pub fn of_load(before: Option<u64>, after: Option<u64>, full: Option<u64>) -> Self {
        match (before, after, full) {
            (Some(b), Some(a), _) if b != a => Baseline::Stale,
            (Some(_), Some(_), Some(full)) => Baseline::Known(full),
            _ => Baseline::Unknown,
        }
    }
}

/// Change detection with debounce, separate from the polling loop so it can
/// be tested without a runtime clock.
#[derive(Debug, Default)]
struct Detector {
    baseline: Baseline,
    pending: Option<u64>,
}

impl Detector {
    /// Feed the current hash; `true` once a change has been stable for one
    /// round. Without a known baseline the first hash only sets it.
    fn update(&mut self, current: u64) -> bool {
        let changed = match self.baseline {
            Baseline::Unknown => {
                self.baseline = Baseline::Known(current);
                return false;
            }
            Baseline::Known(b) => current != b,
            Baseline::Stale => true,
        };
        if !changed {
            self.pending = None;
            return false;
        }
        if self.pending != Some(current) {
            self.pending = Some(current);
            return false;
        }
        self.baseline = Baseline::Known(current);
        self.pending = None;
        true
    }
}

/// Poll `dir` every `interval` and send a replan to the overlord when the
/// layers changed since `baseline` (the state they were loaded in). Runs
/// until the overlord is gone.
pub fn spawn(
    dir: PathBuf,
    secrets: WatchedFiles,
    interval: Duration,
    baseline: Baseline,
    tx: mpsc::Sender<Cmd>,
) -> JoinHandle<()> {
    let interval = clamp_interval(interval);
    tokio::spawn(async move {
        let mut detector = Detector {
            baseline,
            pending: None,
        };
        if baseline != Baseline::Unknown {
            // Every change since the load is seen.
            info!(dir = %dir.display(), ?interval, "watching layers");
        }
        let mut reader = PeriodicRead::default();
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let d = dir.clone();
            let s = secrets.get();
            let hash = match reader.run(move || layers_hash(&d, &s)).await {
                ReadOutcome::Done(Some(h)) => h,
                ReadOutcome::TimedOut => {
                    warn!(
                        dir = %dir.display(),
                        "layers could not be read within {}s (hanging file system?); \
                         not checking them until the read returns",
                        FILE_IO_TIMEOUT.as_secs()
                    );
                    continue;
                }
                // Unreadable this round, or the last read still hangs.
                _ => continue,
            };
            let first = detector.baseline == Baseline::Unknown;
            if !detector.update(hash) {
                if first {
                    // Logged once the baseline is taken: from now on every
                    // change is seen.
                    info!(dir = %dir.display(), ?interval, "watching layers");
                }
                continue;
            }
            info!(dir = %dir.display(), "layers changed, replanning");
            let (reply, rx) = oneshot::channel();
            if tx.send(Cmd::ReloadLayers { reply }).await.is_err() {
                return; // overlord gone
            }
            // The overlord logs and alerts on errors itself.
            let _ = rx.await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_reacts_once_after_a_stable_round() {
        let mut d = Detector::default();
        assert!(!d.update(1), "first hash is the baseline");
        assert!(!d.update(1));
        assert!(!d.update(2), "new content: wait one round");
        assert!(!d.update(3), "still changing: wait again");
        assert!(d.update(3), "stable: react");
        assert!(!d.update(3), "react only once");
        assert!(!d.update(1), "changed back: wait");
        assert!(!d.update(3), "reverted within a round: nothing");
    }

    #[test]
    fn baseline_of_a_load() {
        use Baseline::*;
        assert_eq!(Baseline::of_load(Some(1), Some(1), Some(9)), Known(9));
        assert_eq!(Baseline::of_load(Some(1), Some(2), Some(9)), Stale);
        assert_eq!(Baseline::of_load(None, Some(1), Some(9)), Unknown);
        assert_eq!(Baseline::of_load(Some(1), Some(1), None), Unknown);
    }

    #[test]
    fn a_known_baseline_sees_a_change_on_the_first_poll() {
        let mut d = Detector {
            baseline: Baseline::Known(1),
            pending: None,
        };
        assert!(!d.update(1), "as loaded");
        assert!(!d.update(2), "changed: wait one round");
        assert!(d.update(2), "stable: react");
        let mut d = Detector {
            baseline: Baseline::Known(1),
            pending: None,
        };
        assert!(
            !d.update(2),
            "changed before the first poll: wait one round"
        );
        assert!(
            d.update(2),
            "and react, instead of taking it as the baseline"
        );
    }

    #[test]
    fn a_stale_baseline_replans_once_stable() {
        let mut d = Detector {
            baseline: Baseline::Stale,
            pending: None,
        };
        assert!(!d.update(5), "wait one round");
        assert!(d.update(5), "replan");
        assert!(!d.update(5), "only once");
    }

    /// The layers change after the load but before the watcher's first
    /// poll: the watcher replans (before, it took the new state as its
    /// baseline and never did).
    #[tokio::test]
    async fn a_change_between_load_and_first_poll_replans() {
        let dir = tempfile::tempdir().unwrap();
        let layer = dir.path().join("001-base.yaml");
        std::fs::write(&layer, "services: {}\n").unwrap();
        let loaded = layers_hash(dir.path(), &[]).unwrap();
        std::fs::write(&layer, "services: {a: {command: x}}\n").unwrap();

        let (tx, mut rx) = mpsc::channel(4);
        let task = spawn(
            dir.path().to_owned(),
            WatchedFiles::default(),
            Duration::from_secs(1),
            Baseline::Known(loaded),
            tx,
        );
        let cmd = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("no replan")
            .expect("channel closed");
        let Cmd::ReloadLayers { reply } = cmd else {
            panic!("not a replan");
        };
        reply.send(Ok(())).unwrap();
        task.abort();
    }

    #[test]
    fn hash_covers_names_and_contents_but_not_kubelet_entries() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let h = || layers_hash(p, &[]).unwrap();
        std::fs::write(p.join("001-base.yaml"), "services: {}\n").unwrap();
        let base = h();

        std::fs::write(p.join("README.txt"), "not a layer").unwrap();
        std::fs::create_dir(p.join("..2026_11_06_10_00_00.123")).unwrap();
        std::os::unix::fs::symlink("..2026_11_06_10_00_00.123", p.join("..data")).unwrap();
        std::fs::write(p.join("..hidden.yaml"), "x").unwrap();
        assert_eq!(h(), base, "only layer files count");

        std::fs::write(p.join("001-base.yaml"), "services: {a: {}}\n").unwrap();
        let changed = h();
        assert_ne!(changed, base, "content counts");

        std::fs::rename(p.join("001-base.yaml"), p.join("002-base.yaml")).unwrap();
        assert_ne!(h(), changed, "the name counts (it sets the order)");
    }

    #[test]
    fn hash_covers_watched_files_of_identities() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("log-relay.hash");
        std::fs::write(&secret, "$6$old$hash\n").unwrap();
        let secrets = [secret.clone()];
        let h = || layers_hash(dir.path(), &secrets).unwrap();
        let before = h();
        assert_ne!(before, layers_hash(dir.path(), &[]).unwrap());

        std::fs::write(&secret, "$6$new$hash\n").unwrap();
        let rotated = h();
        assert_ne!(rotated, before, "a rotated Secret triggers a replan");

        std::fs::remove_file(&secret).unwrap();
        assert_ne!(h(), rotated, "so does a removed one (the load reports it)");
    }

    #[test]
    fn missing_dir_has_its_own_hash() {
        let dir = tempfile::tempdir().unwrap();
        let empty = layers_hash(dir.path(), &[]).unwrap();
        let missing = layers_hash(&dir.path().join("nope"), &[]).unwrap();
        assert_ne!(empty, missing);
    }
}
