// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The HTTP client of HTTP checks and alerts, from their `tls:` and
//! `proxy:` blocks.

use vigil_tls::{ClientTls, PemFile};
use vigil_types::no_proxy::{no_proxy_matches, parse_no_proxy};
use vigil_types::plan::{ProxyConfig, TlsConfig};

/// Add `tls` and `proxy` to `builder` and build the client. Reads the PEM
/// files (absolute paths only): call it off the async runtime.
///
/// Only `proxy` is used: reqwest's own lookup of `HTTPS_PROXY` / `NO_PROXY`
/// in vigild's environment is switched off (alerts pass the variables their
/// `pass-env*` rules allow as `proxy`).
pub fn build_http_client(
    builder: reqwest::ClientBuilder,
    tls: Option<&TlsConfig>,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<reqwest::Client> {
    Ok(configure_http_client(builder, tls, proxy)?.build()?)
}

/// Like [`build_http_client`], but return the builder, for callers that
/// add more before building.
pub fn configure_http_client(
    builder: reqwest::ClientBuilder,
    tls: Option<&TlsConfig>,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<reqwest::ClientBuilder> {
    let builder = match tls {
        Some(t) => ClientTls {
            insecure: t.insecure,
            ca: t.ca.iter().map(|p| PemFile::layer("tls.ca", p)).collect(),
            cert: t.cert.as_deref().map(|p| PemFile::layer("tls.cert", p)),
            key: t.key.as_deref().map(|p| PemFile::layer("tls.key", p)),
        }
        .apply(builder)?,
        None => builder,
    };
    let builder = match proxy {
        Some(p) => {
            let url: reqwest::Url = p
                .url
                .parse()
                .map_err(|e| anyhow::anyhow!("proxy.url {:?}: {e}", p.url))?;
            let no_proxy = parse_no_proxy(p.no_proxy.as_deref());
            builder.proxy(reqwest::Proxy::custom(move |target| {
                let host = target.host_str().unwrap_or("");
                (!no_proxy_matches(host, &no_proxy)).then(|| url.clone())
            }))
        }
        None => builder.no_proxy(),
    };
    Ok(builder)
}
