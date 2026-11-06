// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! HTTP proxy configuration and no-proxy host matching.

use std::path::PathBuf;

use anyhow::Context as _;

// ---------------------------------------------------------------------------
// HttpConfig
// ---------------------------------------------------------------------------

/// Configuration for the HTTP/HTTPS transport.
pub struct HttpConfig {
    /// Skip TLS certificate verification for the server.
    /// Useful for vigild's auto-generated self-signed certificate.
    pub insecure: bool,

    /// HTTP Basic Auth credentials as `"username:password"`.
    ///
    /// Sets `Authorization: Basic …` on every request.
    pub user: Option<String>,

    /// Path to a PEM file with a client certificate for mTLS.
    ///
    /// Must be used together with `key`.
    pub cert: Option<PathBuf>,

    /// Path to a PEM file with the private key matching `cert`.
    pub key: Option<PathBuf>,

    /// Path to a PEM file with one or more CA certificates to trust for the
    /// vigild server TLS connection.  Multiple certs may be concatenated.
    pub cacert: Option<PathBuf>,

    /// HTTP or HTTPS proxy URL.
    ///
    /// If `None`, the environment variables `HTTPS_PROXY`, `ALL_PROXY`, and
    /// `HTTP_PROXY` (checked in that order) are used when present.
    pub proxy: Option<String>,

    /// Path to a PEM file containing one or more CA certificates to trust for
    /// the proxy's TLS connection (e.g. a corporate MITM proxy certificate).
    /// Multiple certificates may be concatenated in the same file.
    pub proxy_cacert: Option<PathBuf>,

    /// Comma-separated list of hosts for which **not** to use a proxy.
    ///
    /// Each entry is matched against the request hostname (port stripped).
    /// An entry matches if it equals the hostname exactly or if the hostname
    /// ends with `.<entry>`. A leading dot on an entry is ignored.
    ///
    /// Example: `"local.com"` matches `local.com`, `local.com:80`, and
    /// `www.local.com`, but **not** `www.notlocal.com`.
    pub no_proxy: Option<String>,
}

// ---------------------------------------------------------------------------
// reqwest client builder
// ---------------------------------------------------------------------------

/// Build a `reqwest::Client` from the given `HttpConfig`.
pub(super) fn build_reqwest_client(config: HttpConfig) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::ClientBuilder::new().danger_accept_invalid_certs(config.insecure);

    // HTTP Basic Auth — inject as a default Authorization header.
    if let Some(user_pass) = &config.user {
        let (user, pass) = user_pass
            .split_once(':')
            .with_context(|| format!("--user must be 'username:password', got: {user_pass}"))?;
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        let header_value = reqwest::header::HeaderValue::from_str(&format!("Basic {encoded}"))
            .context("building Authorization header")?;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::AUTHORIZATION, header_value);
        builder = builder.default_headers(headers);
    }

    // Server CA, proxy CA and client certificate (mTLS). reqwest has no TLS
    // settings of its own for the proxy: its CA applies to the whole client.
    let tls = vigil_tls::ClientTls {
        insecure: config.insecure,
        ca: [
            config
                .cacert
                .as_deref()
                .map(|p| vigil_tls::PemFile::new("--cacert", p)),
            config
                .proxy_cacert
                .as_deref()
                .map(|p| vigil_tls::PemFile::new("--proxy-cacert", p)),
        ]
        .into_iter()
        .flatten()
        .collect(),
        cert: config
            .cert
            .as_deref()
            .map(|p| vigil_tls::PemFile::new("--cert", p)),
        key: config
            .key
            .as_deref()
            .map(|p| vigil_tls::PemFile::new("--key", p)),
    };
    builder = tls.apply(builder)?;

    // Resolve effective proxy: explicit arg > env vars (HTTPS first).
    let effective_proxy = config.proxy.or_else(|| {
        for var in &[
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "HTTP_PROXY",
            "http_proxy",
        ] {
            if let Ok(v) = std::env::var(var)
                && !v.is_empty()
            {
                return Some(v);
            }
        }
        None
    });

    if let Some(proxy_url) = effective_proxy {
        let proxy_uri: reqwest::Url = proxy_url
            .parse()
            .with_context(|| format!("invalid proxy URL: {}", proxy_url))?;

        let no_proxy_entries = vigil_types::no_proxy::parse_no_proxy(config.no_proxy.as_deref());

        // Proxy::custom lets us implement our own no_proxy matching.
        // In reqwest 0.12, adding an explicit proxy disables automatic
        // env-var proxy detection, so returning None gives a direct connection.
        let proxy = reqwest::Proxy::custom(move |url| {
            let host = url.host_str().unwrap_or("");
            if vigil_types::no_proxy::no_proxy_matches(host, &no_proxy_entries) {
                None
            } else {
                Some(proxy_uri.clone())
            }
        });

        builder = builder.proxy(proxy);
    }

    Ok(builder.build()?)
}

#[cfg(test)]
mod tests;
