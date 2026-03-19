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
    let mut builder = reqwest::ClientBuilder::new()
        .danger_accept_invalid_certs(config.insecure);

    // Trust custom CA certificates (e.g. proxy's self-signed CA).
    // The PEM file may contain multiple concatenated certificates.
    if let Some(ca_path) = &config.proxy_cacert {
        let pem = std::fs::read(ca_path)
            .with_context(|| format!("reading proxy CA cert: {}", ca_path.display()))?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem)
            .context("parsing proxy CA cert bundle")?;
        for cert in certs {
            builder = builder.add_root_certificate(cert);
        }
    }

    // Resolve effective proxy: explicit arg > env vars (HTTPS first).
    let effective_proxy = config.proxy.or_else(|| {
        for var in &[
            "HTTPS_PROXY", "https_proxy",
            "ALL_PROXY",   "all_proxy",
            "HTTP_PROXY",  "http_proxy",
        ] {
            if let Ok(v) = std::env::var(var) {
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
        None
    });

    if let Some(proxy_url) = effective_proxy {
        let proxy_uri: reqwest::Url = proxy_url
            .parse()
            .with_context(|| format!("invalid proxy URL: {}", proxy_url))?;

        let no_proxy_entries = parse_no_proxy(config.no_proxy.as_deref());

        // Proxy::custom lets us implement our own no_proxy matching.
        // In reqwest 0.12, adding an explicit proxy disables automatic
        // env-var proxy detection, so returning None gives a direct connection.
        let proxy = reqwest::Proxy::custom(move |url| {
            let host = url.host_str().unwrap_or("");
            if no_proxy_matches(host, &no_proxy_entries) {
                None
            } else {
                Some(proxy_uri.clone())
            }
        });

        builder = builder.proxy(proxy);
    }

    Ok(builder.build()?)
}

// ---------------------------------------------------------------------------
// no_proxy matching
// ---------------------------------------------------------------------------

/// Parse a comma-separated no_proxy string into a list of lowercase entries.
pub(super) fn parse_no_proxy(s: Option<&str>) -> Vec<String> {
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
/// `host` may include a port (`host:8080`); the port is stripped before
/// matching. Each `entry` in `entries` matches:
/// - the hostname exactly (`"local.com"` → `"local.com"`)
/// - any subdomain (`"local.com"` → `"www.local.com"`, `"a.b.local.com"`)
///
/// A leading `.` on an entry is ignored (`.local.com` ≡ `local.com`).
pub(super) fn no_proxy_matches(host: &str, entries: &[String]) -> bool {
    if entries.is_empty() {
        return false;
    }

    // Strip port if present (e.g. "host:8080" → "host").
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(s: &str) -> Vec<String> {
        parse_no_proxy(Some(s))
    }

    #[test]
    fn exact_match() {
        assert!(no_proxy_matches("local.com", &entries("local.com")));
    }

    #[test]
    fn port_stripped() {
        assert!(no_proxy_matches("local.com:80", &entries("local.com")));
        assert!(no_proxy_matches("local.com:8443", &entries("local.com")));
    }

    #[test]
    fn subdomain() {
        assert!(no_proxy_matches("www.local.com", &entries("local.com")));
        assert!(no_proxy_matches("a.b.local.com", &entries("local.com")));
    }

    #[test]
    fn no_false_suffix() {
        assert!(!no_proxy_matches("www.notlocal.com", &entries("local.com")));
        assert!(!no_proxy_matches("notlocal.com", &entries("local.com")));
    }

    #[test]
    fn leading_dot_entry() {
        assert!(no_proxy_matches("local.com", &entries(".local.com")));
        assert!(no_proxy_matches("www.local.com", &entries(".local.com")));
        assert!(!no_proxy_matches("www.notlocal.com", &entries(".local.com")));
    }

    #[test]
    fn multiple_entries() {
        let e = entries("internal.corp, .dev.local, 127.0.0.1");
        assert!(no_proxy_matches("internal.corp", &e));
        assert!(no_proxy_matches("api.dev.local", &e));
        assert!(no_proxy_matches("127.0.0.1", &e));
        assert!(!no_proxy_matches("external.corp", &e));
    }

    #[test]
    fn empty_list() {
        assert!(!no_proxy_matches("anything.com", &[]));
    }

    #[test]
    fn case_insensitive() {
        assert!(no_proxy_matches("LOCAL.COM", &entries("local.com")));
        assert!(no_proxy_matches("WWW.Local.Com", &entries("local.com")));
    }
}
