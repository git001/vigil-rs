// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `--watch-layers`: replan automatically when the layer files change, e.g.
//! after a ConfigMap update.
//!
//! Polling, not inotify, for the same reasons as `watch-files`: the kubelet
//! swaps the ConfigMap's `..data` symlink atomically, and polling also works
//! on NFS and overlayfs. One hash covers the names and contents of all layer
//! files ([`is_layer_file`]); the kubelet's own `..` entries are skipped.
//!
//! A change triggers a replan once it has been stable for one polling round,
//! so a ConfigMap with several files is applied once. The replan itself
//! validates first: invalid layers keep the running plan and send the alert
//! `vigild`/`layers`/`invalid`. A failed replan is not retried; only the
//! next change of the files triggers a new one.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use super::Cmd;
use super::plan::is_layer_file;
use crate::duration::clamp_interval;
use crate::file_io::{ContentHasher, FILE_IO_TIMEOUT, PeriodicRead, ReadOutcome, hash_into};

/// Hash over the names and contents of the layer files in `dir`, in the
/// order `load_plan` reads them. A missing directory has its own hash (it
/// loads as an empty plan). `None`: the directory or a file could not be
/// read; the round is skipped.
fn layers_hash(dir: &Path) -> Option<u64> {
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
    Some(h.finish())
}

/// Change detection with debounce, separate from the polling loop so it can
/// be tested without a runtime clock.
#[derive(Debug, Default)]
struct Detector {
    baseline: Option<u64>,
    pending: Option<u64>,
}

impl Detector {
    /// Feed the current hash; `true` once a change has been stable for one
    /// round. The first hash only sets the baseline.
    fn update(&mut self, current: u64) -> bool {
        let Some(baseline) = self.baseline else {
            self.baseline = Some(current);
            return false;
        };
        if current == baseline {
            self.pending = None;
            return false;
        }
        if self.pending != Some(current) {
            self.pending = Some(current);
            return false;
        }
        self.baseline = Some(current);
        self.pending = None;
        true
    }
}

/// Poll `dir` every `interval` and send a replan to the overlord when the
/// layers changed. Runs until the overlord is gone.
pub fn spawn(dir: PathBuf, interval: Duration, tx: mpsc::Sender<Cmd>) -> JoinHandle<()> {
    let interval = clamp_interval(interval);
    tokio::spawn(async move {
        info!(dir = %dir.display(), ?interval, "watching layers");
        let mut detector = Detector::default();
        let mut reader = PeriodicRead::default();
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let d = dir.clone();
            let hash = match reader.run(move || layers_hash(&d)).await {
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
            if !detector.update(hash) {
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
    fn hash_covers_names_and_contents_but_not_kubelet_entries() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let h = || layers_hash(p).unwrap();
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
    fn missing_dir_has_its_own_hash() {
        let dir = tempfile::tempdir().unwrap();
        let empty = layers_hash(dir.path()).unwrap();
        let missing = layers_hash(&dir.path().join("nope")).unwrap();
        assert_ne!(empty, missing);
    }
}
