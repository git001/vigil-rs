// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use super::cert::load_pem_chain;

/// Configuration for an outbound HTTP(S) client (used by alerts and checks).
pub struct HttpClientConfig<'a> {
    /// Skip TLS certificate verification.
    pub insecure: bool,
    /// PEM CA file (or chain) to verify the server's TLS.
    pub ca: Option<&'a std::path::Path>,
    /// Proxy URL. `None`: no proxy. vigild's own proxy variables are not
    /// read here; alerts pass the ones their `pass-env*` policy allows.
    pub proxy: Option<&'a str>,
    /// PEM CA file (or chain) to verify the proxy's TLS.
    pub proxy_ca: Option<&'a std::path::Path>,
    /// Comma-separated no-proxy host list (e.g. `"internal.corp, .dev.local"`).
    pub no_proxy: Option<&'a str>,
}

/// Build a `reqwest::Client` from `HttpClientConfig`. Only the given proxy
/// is used: reqwest's own lookup of `HTTPS_PROXY` / `NO_PROXY` in vigild's
/// environment is switched off.
pub fn build_http_client(config: HttpClientConfig<'_>) -> anyhow::Result<reqwest::Client> {
    let mut b = reqwest::ClientBuilder::new().danger_accept_invalid_certs(config.insecure);

    if let Some(ca_path) = config.ca {
        for cert in load_pem_chain(ca_path)? {
            b = b.add_root_certificate(cert);
        }
    }

    if let Some(ca_path) = config.proxy_ca {
        for cert in load_pem_chain(ca_path)? {
            b = b.add_root_certificate(cert);
        }
    }

    if let Some(proxy_url) = config.proxy {
        let proxy_uri: reqwest::Url = proxy_url
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid proxy URL {proxy_url:?}: {e}"))?;
        let no_proxy_entries = parse_no_proxy(config.no_proxy);
        let proxy = reqwest::Proxy::custom(move |url| {
            let host = url.host_str().unwrap_or("");
            if no_proxy_matches(host, &no_proxy_entries) {
                None
            } else {
                Some(proxy_uri.clone())
            }
        });
        b = b.proxy(proxy);
    } else {
        b = b.no_proxy();
    }

    Ok(b.build()?)
}

/// Parse a comma-separated `no_proxy` string into lowercase host entries.
pub fn parse_no_proxy(s: Option<&str>) -> Vec<String> {
    match s {
        None => vec![],
        Some(s) => s
            .split(',')
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .collect(),
    }
}

/// Returns `true` if `host` should bypass the proxy.
///
/// Port is stripped before matching. A leading `.` on an entry is ignored.
/// Example: `"local.com"` matches `local.com`, `local.com:80`, `sub.local.com`.
pub fn no_proxy_matches(host: &str, entries: &[String]) -> bool {
    if entries.is_empty() {
        return false;
    }
    let bare = host
        .rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
        .map(|(h, _)| h)
        .unwrap_or(host)
        .to_ascii_lowercase();
    entries.iter().any(|entry| {
        let e = entry.strip_prefix('.').unwrap_or(entry.as_str());
        bare == e
            || (bare.len() > e.len()
                && bare.as_bytes()[bare.len() - e.len() - 1] == b'.'
                && bare.ends_with(e))
    })
}
