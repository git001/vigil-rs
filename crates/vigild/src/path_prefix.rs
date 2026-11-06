// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The `--path-prefix` directory: the single place where vigild creates
//! files at runtime (control socket, later sd_notify sockets).
//!
//! The directory is checked strictly at startup and vigild refuses to run
//! if it is unsafe. There is no fallback chain: the operator chooses the
//! path (e.g. `/run/vigil` with root, `/tmp/vigil` on OpenShift where `/run`
//! is a root-owned tmpfs).

use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::Context;
use nix::unistd::{AccessFlags, Uid, access};
use tracing::info;

pub use vigil_types::paths::{DEFAULT_PATH_PREFIX, SOCKET_NAME, socket_path};

/// Subdirectory for the per-start sd_notify sockets (`ready: notify`).
pub const NOTIFY_DIR: &str = "notify";

/// Mode for directories vigild creates itself (the prefix's last component
/// and `notify/`): owner rwx plus the search bit for everyone, never read or
/// write. Services running as another `user:` must be able to reach their
/// notify socket; listing stays impossible and every socket inside has its
/// own restrictive mode (control socket: `--socket-mode` plus the peer-UID
/// check; notify sockets: 0600, owned by the service user).
pub const CREATED_DIR_MODE: u32 = 0o711;

/// Directory holding the sd_notify sockets.
pub fn notify_dir(prefix: &Path) -> PathBuf {
    prefix.join(NOTIFY_DIR)
}

/// Make sure `prefix` is a safe directory owned by us, creating its last
/// component if it is missing.
///
/// Accepted:
/// - an absolute path to a directory (not a symlink),
/// - owned by vigild's effective UID,
/// - not writable by others (`o+w`); `g+w` is fine because OpenShift images
///   commonly use `chmod g=u` for GID 0,
/// - writable and searchable by vigild.
///
/// Only the last component is created, with [`CREATED_DIR_MODE`]; a missing
/// parent is an error. This lets `--path-prefix /data/vigil` work on top of
/// an `emptyDir` mounted at `/data` (root-owned, mode 0777, which itself
/// would be rejected).
///
/// The `notify/` subdirectory is created (and checked) the same way.
pub fn prepare(prefix: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        prefix.is_absolute(),
        "--path-prefix {prefix:?} must be an absolute path"
    );

    ensure_own_dir(prefix, "--path-prefix")?;
    ensure_own_dir(&notify_dir(prefix), "notify directory")
}

/// Create `dir` if it is missing (only its last component), then check that
/// it is vigild's own directory. `what` names it in errors.
fn ensure_own_dir(dir: &Path, what: &str) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => create_last(dir, what)?,
        Err(e) => return Err(e).with_context(|| format!("inspecting {what} {dir:?}")),
    }
    validate(dir, what)
}

fn create_last(prefix: &Path, what: &str) -> anyhow::Result<()> {
    let parent = prefix
        .parent()
        .with_context(|| format!("{what} {prefix:?} has no parent directory"))?;
    let parent_md = std::fs::metadata(parent).with_context(|| {
        format!(
            "{what} {prefix:?}: parent directory {parent:?} does not exist \
             (vigild only creates the last path component)"
        )
    })?;
    anyhow::ensure!(
        parent_md.is_dir(),
        "{what} {prefix:?}: parent {parent:?} is not a directory"
    );

    let mode = CREATED_DIR_MODE;
    match std::fs::DirBuilder::new().mode(mode).create(prefix) {
        Ok(()) => {
            // DirBuilder honours the umask; set the exact mode explicitly.
            std::fs::set_permissions(prefix, std::fs::Permissions::from_mode(mode))
                .with_context(|| format!("chmod {mode:o} {prefix:?}"))?;
            info!(path = ?prefix, mode = format!("{mode:o}"), "created path prefix directory");
            Ok(())
        }
        // Created concurrently by someone else: validate() decides.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e).with_context(|| {
            format!(
                "creating {what} {prefix:?} (is {parent:?} writable for uid {}?)",
                Uid::effective()
            )
        }),
    }
}

fn validate(prefix: &Path, what: &str) -> anyhow::Result<()> {
    let md = std::fs::symlink_metadata(prefix)
        .with_context(|| format!("inspecting {what} {prefix:?}"))?;
    let euid = Uid::effective().as_raw();
    let mode = md.permissions().mode() & 0o7777;
    let hint = || {
        format!(
            "expected a directory owned by uid {euid}, not a symlink, not writable by others. \
             Found: owner uid {}, mode {:o}{}. Hint: point --path-prefix at a not yet existing \
             subdirectory of a writable directory (e.g. /tmp/vigil) and vigild creates it",
            md.uid(),
            mode,
            if md.file_type().is_symlink() {
                ", symlink"
            } else {
                ""
            },
        )
    };

    anyhow::ensure!(
        !md.file_type().is_symlink() && md.is_dir(),
        "{what} {prefix:?} rejected: {}",
        hint()
    );
    anyhow::ensure!(
        md.uid() == euid,
        "{what} {prefix:?} rejected (wrong owner): {}",
        hint()
    );
    anyhow::ensure!(
        mode & 0o002 == 0,
        "{what} {prefix:?} rejected (writable by others): {}",
        hint()
    );
    access(prefix, AccessFlags::W_OK | AccessFlags::X_OK)
        .with_context(|| format!("{what} {prefix:?} is not writable/searchable for uid {euid}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode_of(p: &Path) -> u32 {
        std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn creates_missing_last_component_and_notify_dir_with_0711() {
        let base = tempfile::tempdir().unwrap();
        let p = base.path().join("vigil");
        prepare(&p).unwrap();
        assert!(p.is_dir());
        assert_eq!(mode_of(&p), 0o711);
        assert_eq!(mode_of(&notify_dir(&p)), 0o711);
    }

    #[test]
    fn symlinked_notify_dir_is_rejected() {
        let base = tempfile::tempdir().unwrap();
        let p = base.path().join("vigil");
        std::fs::create_dir(&p).unwrap();
        std::os::unix::fs::symlink(base.path(), notify_dir(&p)).unwrap();
        let err = prepare(&p).unwrap_err().to_string();
        assert!(err.contains("symlink"), "{err}");
    }

    #[test]
    fn missing_parent_is_an_error() {
        let base = tempfile::tempdir().unwrap();
        let p = base.path().join("a/b");
        let err = prepare(&p).unwrap_err().to_string();
        assert!(
            err.contains("only creates the last path component"),
            "{err}"
        );
        assert!(!base.path().join("a").exists());
    }

    #[test]
    fn existing_own_dir_is_accepted_and_left_alone() {
        let base = tempfile::tempdir().unwrap();
        let p = base.path().join("vigil");
        std::fs::create_dir(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o770)).unwrap();
        prepare(&p).unwrap();
        assert_eq!(
            mode_of(&p),
            0o770,
            "g+w allowed (OpenShift g=u), mode untouched"
        );
    }

    #[test]
    fn world_writable_dir_is_rejected() {
        let base = tempfile::tempdir().unwrap();
        let p = base.path().join("vigil");
        std::fs::create_dir(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o777)).unwrap();
        let err = prepare(&p).unwrap_err().to_string();
        assert!(err.contains("writable by others"), "{err}");
        assert!(err.contains("mode 777"), "{err}");
    }

    #[test]
    fn sticky_tmp_like_dir_is_rejected() {
        let base = tempfile::tempdir().unwrap();
        let p = base.path().join("tmp");
        std::fs::create_dir(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o1777)).unwrap();
        assert!(prepare(&p).is_err());
    }

    #[test]
    fn symlink_is_rejected() {
        let base = tempfile::tempdir().unwrap();
        let real = base.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = base.path().join("vigil");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = prepare(&link).unwrap_err().to_string();
        assert!(err.contains("symlink"), "{err}");
    }

    #[test]
    fn regular_file_is_rejected() {
        let base = tempfile::tempdir().unwrap();
        let p = base.path().join("vigil");
        std::fs::write(&p, b"x").unwrap();
        assert!(prepare(&p).is_err());
    }

    #[test]
    fn foreign_owner_is_rejected() {
        if Uid::effective().is_root() {
            return; // as root we cannot easily get a dir we do not own
        }
        // Any system directory owned by someone else (sandboxes and user
        // namespaces may remap `/` to our own uid, so try several).
        let me = Uid::effective().as_raw();
        let Some(foreign) = ["/", "/usr", "/proc", "/dev", "/etc"]
            .into_iter()
            .map(Path::new)
            .find(|p| {
                std::fs::symlink_metadata(p)
                    .map(|m| m.is_dir() && m.uid() != me && m.permissions().mode() & 0o002 == 0)
                    .unwrap_or(false)
            })
        else {
            eprintln!("skipped: no foreign-owned directory found");
            return;
        };
        let err = prepare(foreign).unwrap_err().to_string();
        assert!(err.contains("wrong owner"), "{foreign:?}: {err}");
    }

    #[test]
    fn relative_path_is_rejected() {
        assert!(prepare(Path::new("run/vigil")).is_err());
    }
}
