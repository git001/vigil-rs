// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Reading (secret) files: the `file(path[, default])` template function and
//! `env-file`.

use std::io::Read;
use std::path::Path;

use minijinja::{Error, ErrorKind};

/// Largest file `file()` reads.
pub(super) const MAX_FILE: u64 = 64 * 1024;

/// Read `path` as text: a regular file (symlinks are followed, Kubernetes
/// secret volumes consist of them), at most `max` bytes, UTF-8 without NUL
/// bytes. `Ok(None)` if it does not exist. Errors name the path, never the
/// content.
pub(super) fn read_text(path: &str, max: u64) -> Result<Option<String>, String> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(format!("{path:?}: path must be absolute"));
    }
    let mut f = match std::fs::File::open(p) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{path:?}: {e}")),
    };
    // Check the opened file, not the path: no race with a swapped symlink.
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
    if buf.contains(&0) {
        return Err(format!("{path:?}: contains a NUL byte"));
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|_| format!("{path:?}: not valid UTF-8"))
}

/// Content of `path` without one trailing newline; `default` if the file does
/// not exist. Every other problem is an error.
pub(super) fn file(path: String, default: Option<String>) -> Result<String, Error> {
    let fail = |msg: String| Error::new(ErrorKind::InvalidOperation, format!("file({msg})"));
    match read_text(&path, MAX_FILE).map_err(fail)? {
        Some(mut s) => {
            if s.ends_with('\n') {
                s.pop();
            }
            Ok(s)
        }
        None => default.ok_or_else(|| fail(format!("{path:?}: No such file or directory"))),
    }
}
