// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Runtime paths shared by vigild and the vigil CLI.

use std::path::{Path, PathBuf};

/// Default `--path-prefix` / `VIGIL_PATH_PREFIX`.
pub const DEFAULT_PATH_PREFIX: &str = "/run/vigil";

/// File name of the control socket inside the prefix.
pub const SOCKET_NAME: &str = "vigild.sock";

/// Default control-socket path for a given prefix.
pub fn socket_path(prefix: &Path) -> PathBuf {
    prefix.join(SOCKET_NAME)
}
