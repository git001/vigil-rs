// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

pub mod alert;
pub mod api;
pub mod check;
pub mod duration;
pub mod identity;
pub mod logs;
pub mod metrics;
pub mod overlord;
pub mod process_util;
pub mod reaper;
pub mod server;
pub mod service;
pub mod state;
pub mod tls;

pub mod testutil;

/// Install the `aws-lc-rs` rustls crypto provider as the process default.
///
/// Uses a `Once` guard so it is safe to call from multiple tests or from both
/// library init and `main`. Subsequent calls after the first are no-ops.
pub fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}
