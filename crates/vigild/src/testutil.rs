// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

/// Alias for [`crate::install_crypto_provider`] — use in test modules.
pub fn init_crypto() {
    crate::install_crypto_provider();
}
