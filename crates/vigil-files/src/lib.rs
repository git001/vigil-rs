// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! How vigil reads the files its configuration names: secrets
//! (`file()`, `password-hash-file`), env files, certificates and keys,
//! credential files.
//!
//! Every read checks the same things:
//! - with [`PathRule::Absolute`] (paths from layer files, where vigild's
//!   working directory is arbitrary) the path must be absolute;
//! - the opened file must be a regular file (symlinks are followed, as in
//!   Kubernetes Secret volumes); the check is on the opened file, so a
//!   swapped symlink cannot race it;
//! - at most `max` bytes;
//! - errors name the path, never the content.
//!
//! The functions block and have no time limit. Code on an async runtime
//! reads through [`off_runtime`] or [`PeriodicRead`] (feature `tokio`), so a
//! dead NFS mount cannot stall it.

use std::io::Read;
use std::path::Path;

#[cfg(feature = "tokio")]
mod blocking_pool;
mod hash;

#[cfg(feature = "tokio")]
pub use blocking_pool::{
    FILE_IO_TIMEOUT, PeriodicRead, ReadOutcome, off_runtime, off_runtime_within,
};
pub use hash::{ContentHasher, content_hash, hash_into};

/// Which paths a setting accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRule {
    /// Only absolute paths (layer files).
    Absolute,
    /// Relative paths too, against the working directory (command-line
    /// options).
    Any,
}

/// The bytes of `path`, or `Ok(None)` if it does not exist. Errors start
/// with the quoted path.
pub fn read(path: &Path, max: u64, rule: PathRule) -> Result<Option<Vec<u8>>, String> {
    if rule == PathRule::Absolute && !path.is_absolute() {
        return Err(format!("{path:?}: path must be absolute"));
    }
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{path:?}: {e}")),
    };
    let meta = f.metadata().map_err(|e| format!("{path:?}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{path:?}: not a regular file"));
    }
    let mut buf = Vec::new();
    (&mut f)
        .take(max + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("{path:?}: {e}"))?;
    if buf.len() as u64 > max {
        return Err(format!("{path:?}: larger than {} KiB", max / 1024));
    }
    Ok(Some(buf))
}

/// Like [`read`], as text: UTF-8 without NUL bytes.
pub fn read_text(path: &Path, max: u64, rule: PathRule) -> Result<Option<String>, String> {
    let Some(buf) = read(path, max, rule)? else {
        return Ok(None);
    };
    if buf.contains(&0) {
        return Err(format!("{path:?}: contains a NUL byte"));
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|_| format!("{path:?}: not valid UTF-8"))
}

/// Like [`read`], but a missing file is an error too.
pub fn read_existing(path: &Path, max: u64, rule: PathRule) -> Result<Vec<u8>, String> {
    read(path, max, rule)?.ok_or_else(|| format!("{path:?}: No such file or directory"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_regular_files_and_reports_missing_ones() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("pw");
        std::fs::write(&p, "MY-pw1!\n").unwrap();
        assert_eq!(
            read_text(&p, 64, PathRule::Absolute).unwrap().as_deref(),
            Some("MY-pw1!\n")
        );
        let missing = dir.path().join("missing");
        assert_eq!(read(&missing, 64, PathRule::Absolute).unwrap(), None);
        let err = read_existing(&missing, 64, PathRule::Any).unwrap_err();
        assert!(err.ends_with(": No such file or directory"), "{err}");
    }

    #[test]
    fn follows_symlinks_like_a_secret_volume() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("..data");
        std::fs::create_dir(&data).unwrap();
        std::fs::write(data.join("pw"), "MY-pw1!").unwrap();
        let link = dir.path().join("pw");
        std::os::unix::fs::symlink("..data/pw", &link).unwrap();
        assert_eq!(
            read_text(&link, 64, PathRule::Absolute).unwrap().as_deref(),
            Some("MY-pw1!")
        );
    }

    #[test]
    fn path_rule_decides_about_relative_paths() {
        let err = read(Path::new("run/secrets/pw"), 64, PathRule::Absolute).unwrap_err();
        assert_eq!(err, "\"run/secrets/pw\": path must be absolute");
        // Relative paths are allowed for command-line options.
        assert_eq!(
            read(Path::new("no/such/file"), 64, PathRule::Any).unwrap(),
            None
        );
    }

    #[test]
    fn rejects_non_files_large_files_and_bad_text_without_the_content() {
        let dir = tempfile::tempdir().unwrap();
        let err = read(dir.path(), 64, PathRule::Absolute).unwrap_err();
        assert!(err.ends_with("not a regular file"), "{err}");

        let big = dir.path().join("big");
        std::fs::write(&big, vec![b'x'; 2049]).unwrap();
        let err = read(&big, 2048, PathRule::Absolute).unwrap_err();
        assert!(err.ends_with("larger than 2 KiB"), "{err}");

        let nul = dir.path().join("nul");
        std::fs::write(&nul, b"MY-pw1!\0").unwrap();
        let err = read_text(&nul, 64, PathRule::Absolute).unwrap_err();
        assert!(err.ends_with("contains a NUL byte"), "{err}");
        assert!(!err.contains("MY-pw1!"), "{err}");

        let bin = dir.path().join("bin");
        std::fs::write(&bin, [0xff, 0xfe]).unwrap();
        let err = read_text(&bin, 64, PathRule::Absolute).unwrap_err();
        assert!(err.ends_with("not valid UTF-8"), "{err}");
    }
}
