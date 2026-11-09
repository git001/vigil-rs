// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Whether building a child's start may block, and running it off the
//! runtime only then.

pub use vigil_files::{FILE_IO_TIMEOUT, off_runtime};

use super::is_template;

/// Whether building a child's environment (and command) may block: an
/// `env-file` or a template (`file()`) reads files, a user switch looks the
/// user up (NSS, possibly over the network). Otherwise it is plain CPU work.
pub fn may_block<'a>(
    env_files: &[String],
    values: impl IntoIterator<Item = &'a str>,
    user_switch: bool,
) -> bool {
    user_switch || !env_files.is_empty() || values.into_iter().any(is_template)
}

/// [`off_runtime`] when `blocking`, else `f` right here. Most exec checks
/// and many services read nothing, and each run off the runtime would take
/// a blocking thread, which tokio keeps for 10 s and which costs about
/// 2 MiB.
pub async fn off_runtime_if<T: Send + 'static>(
    blocking: bool,
    what: &str,
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    if blocking {
        off_runtime(what, f).await
    } else {
        f()
    }
}
