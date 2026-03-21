// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

mod alert;
mod check;
mod layer;
mod service;
#[cfg(test)]
mod tests;

pub use self::alert::{AlertConfig, AlertFormat, AlertsBlock};
pub use self::check::{CheckConfig, CheckLevel, ExecCheck, HttpCheck, TcpCheck};
pub use self::layer::{Layer, Plan};
pub use self::service::{LogsForward, LogsPushFormat, OnExit, ServiceConfig, Startup};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Override semantics (shared by service, check, and alert merge logic)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Override {
    #[default]
    Merge,
    Replace,
}
