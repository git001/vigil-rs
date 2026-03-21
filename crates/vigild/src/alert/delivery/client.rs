// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use reqwest::Client;
use tracing::error;
use vigil_types::plan::AlertConfig;

pub(crate) fn build_client(cfg: &AlertConfig) -> Client {
    let config = crate::tls::HttpClientConfig {
        insecure: cfg.tls_insecure,
        ca: cfg.tls_ca.as_deref(),
        proxy: cfg.proxy.as_deref(),
        proxy_ca: cfg.proxy_ca.as_deref(),
        no_proxy: cfg.no_proxy.as_deref(),
    };
    match crate::tls::build_http_client(config) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "alert: failed to build HTTP client, falling back to default");
            Client::new()
        }
    }
}
