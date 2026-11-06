// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Reading (secret) files: the `file(path[, default])` template function and
//! `env-file`.

use std::path::Path;

use minijinja::{Error, ErrorKind};

/// Largest file `file()` reads.
pub(super) const MAX_FILE: u64 = 64 * 1024;

/// Read `path` as text with vigil's file handling ([`vigil_files`]): an
/// absolute path, a regular file, at most `max` bytes, UTF-8 without NUL
/// bytes. `Ok(None)` if it does not exist. Errors name the path, never the
/// content.
pub(super) fn read_text(path: &str, max: u64) -> Result<Option<String>, String> {
    vigil_files::read_text(Path::new(path), max, vigil_files::PathRule::Absolute)
}

/// Content of `path` without one trailing newline. A missing or empty file
/// (only a newline counts as empty) gives `default`, or is an error without
/// one: an empty secret is almost always a mistake (wrong key, unsynced
/// Secret). Every other problem is an error.
pub(super) fn file(
    reads: &super::Reads,
    path: String,
    default: Option<String>,
) -> Result<String, Error> {
    let fail = |msg: String| Error::new(ErrorKind::InvalidOperation, format!("file({msg})"));
    let content = read_text(&path, MAX_FILE).map_err(fail)?;
    reads.record(&path, content.as_deref());
    match content {
        Some(mut s) => {
            if s.ends_with('\n') {
                s.pop();
            }
            if s.is_empty() {
                return default.ok_or_else(|| fail(format!("{path:?}: is empty")));
            }
            Ok(s)
        }
        None => default.ok_or_else(|| fail(format!("{path:?}: No such file or directory"))),
    }
}
