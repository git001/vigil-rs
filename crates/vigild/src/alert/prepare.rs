// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Alerts prepared while the layers load: `env:` values resolved, an HTTP
//! client per alert.

use anyhow::Context;
use indexmap::IndexMap;
use reqwest::Client;
use vigil_types::plan::AlertConfig;

use super::delivery;

// ---------------------------------------------------------------------------
// Per-alert entry (config + dedicated reqwest client for TLS options)
// ---------------------------------------------------------------------------

pub(super) struct AlertEntry {
    pub(super) name: String,
    /// With `env:VAR` values resolved.
    pub(super) config: AlertConfig,
    pub(super) client: Client,
}

/// What loading the layers prepares alerts against.
pub struct LoadEnv {
    /// vigild's environment, which each alert sees through its `pass-env*`
    /// policy.
    pub parent: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

impl LoadEnv {
    /// vigild's own environment (read once).
    pub fn process() -> Self {
        Self {
            parent: std::env::vars_os().collect(),
        }
    }
}

/// Alerts ready to send: `env:VAR` values resolved, HTTP clients built.
/// Made by [`prepare_alerts`] while the layers are loaded (off the async
/// runtime, as it reads the `tls:` files).
#[derive(Default)]
pub struct PreparedAlerts(pub(super) Vec<AlertEntry>);

/// Prepare every alert of a plan; an error names the alert and the field
/// (an unset or not passed variable, an unreadable `tls.ca`, a bad proxy).
pub fn prepare_alerts(
    alerts: &IndexMap<String, AlertConfig>,
    env: &LoadEnv,
) -> anyhow::Result<PreparedAlerts> {
    alerts
        .iter()
        .map(|(name, cfg)| {
            let (config, client) =
                delivery::prepare(cfg, env).with_context(|| format!("alert {name:?}"))?;
            Ok(AlertEntry {
                name: name.clone(),
                config,
                client,
            })
        })
        .collect::<anyhow::Result<_>>()
        .map(PreparedAlerts)
}
