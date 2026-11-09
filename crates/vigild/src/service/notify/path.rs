// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Where a notify socket goes: its path, the check of `--path-prefix`
//! against the length limit, and removing the file on an error.

use std::path::{Path, PathBuf};

/// Longest path that fits into `sockaddr_un.sun_path` (108 bytes incl. NUL).
pub(super) const SUN_PATH_MAX: usize = 107;

/// Removes the socket file when dropped, unless disarmed.
pub(super) struct RemoveOnDrop(pub(super) Option<PathBuf>);

impl RemoveOnDrop {
    pub(super) fn disarm(mut self) -> PathBuf {
        self.0.take().expect("armed")
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Hex digits of the random part of a socket name.
const RANDOM_DIGITS: usize = 16;

pub(super) fn random_suffix() -> String {
    let n = uuid::Uuid::new_v4().as_u128() as u64;
    format!("{n:0RANDOM_DIGITS$x}")
}

/// Whether notify sockets fit into `sun_path` in `dir`: the shortest
/// name, `<dir>/<16 hex digits>.sock`, must. Checked when the layers are
/// loaded, so that a too long `--path-prefix` stops the start or the
/// replan instead of every start of a `ready: notify` service.
pub fn check_dir(dir: &Path) -> anyhow::Result<()> {
    let shortest = dir.join(format!("{}.sock", "0".repeat(RANDOM_DIGITS)));
    let len = shortest.as_os_str().len();
    let max_dir = SUN_PATH_MAX - (shortest.as_os_str().len() - dir.as_os_str().len());
    anyhow::ensure!(
        len <= SUN_PATH_MAX,
        "notify socket paths in {dir:?} are at least {len} bytes, the limit for Unix sockets \
         is {SUN_PATH_MAX}: shorten --path-prefix (the notify directory may have at most \
         {max_dir} bytes)"
    );
    Ok(())
}

/// `<dir>/<service>-<rand>.sock`, or `<dir>/<rand>.sock` if that would not
/// fit into `sun_path` (long service names).
pub(super) fn socket_path(dir: &Path, service: &str, rand: &str) -> PathBuf {
    let named = dir.join(format!("{service}-{rand}.sock"));
    if named.as_os_str().len() <= SUN_PATH_MAX {
        named
    } else {
        dir.join(format!("{rand}.sock"))
    }
}
