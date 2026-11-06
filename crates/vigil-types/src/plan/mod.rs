// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

/// Layer merge: copy every listed `Option` field the overlay sets.
/// `merge_opt!(base, overlay, [command, user])`
macro_rules! merge_opt {
    ($base:expr, $overlay:expr, [$($f:ident),* $(,)?]) => {
        $(
            if $overlay.$f.is_some() {
                $base.$f = $overlay.$f.clone();
            }
        )*
    };
}

mod alert;
mod check;
mod layer;
mod service;
#[cfg(test)]
mod tests;
mod tls;

pub use self::alert::{AlertConfig, AlertFormat, AlertsBlock};
pub use self::check::{CheckConfig, CheckLevel, ExecCheck, HttpCheck, TcpCheck};
pub use self::layer::{Layer, Plan};
pub use self::service::{
    FileChangeAction, LogsForward, LogsPushFormat, OnExit, ReadyMode, ServiceConfig, ServiceType,
    Startup, WatchFiles,
};
pub use self::tls::{ProxyConfig, TlsConfig};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Override semantics (shared by service, check, and alert merge logic)
// ---------------------------------------------------------------------------

/// Layer merge of a list: append the overlay's entries that are not in it
/// yet (order kept).
pub(super) fn append_unique(base: &mut Vec<String>, overlay: &[String]) {
    for v in overlay {
        if !base.contains(v) {
            base.push(v.clone());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Override {
    #[default]
    Merge,
    Replace,
}
