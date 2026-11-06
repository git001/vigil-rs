// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The `tls:` and `proxy:` blocks of outgoing HTTP(S) connections (HTTP
//! checks, alerts).
//!
//! ```yaml
//! tls:
//!   ca: /etc/vigil/ca/internal-ca.pem
//!   cert: /run/secrets/vigil/client.crt
//!   key: /run/secrets/vigil/client.key
//! proxy:
//!   url: http://proxy.example.com:3128
//!   no-proxy: "internal.example.com, .svc.cluster.local"
//! ```
//!
//! A later layer replaces a whole block, never single keys of it.

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};

/// How the server (and an HTTPS proxy) is verified, and the client
/// certificate. All paths must be absolute.
///
/// The CAs and `insecure` apply to the whole connection, to an HTTPS proxy
/// as well as to the server: the HTTP client has no separate TLS settings
/// for the proxy. A proxy with its own CA therefore adds that CA to `ca`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TlsConfig {
    /// PEM files with CA certificates, in addition to the system CAs: one
    /// path or a list. Each file may hold several certificates.
    #[serde(
        default,
        deserialize_with = "one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub ca: Vec<PathBuf>,
    /// PEM file with the client certificate (and its chain); needs `key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert: Option<PathBuf>,
    /// PEM file with the private key of `cert`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<PathBuf>,
    /// Do not verify certificates at all (server and proxy).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub insecure: bool,
}

/// The proxy for HTTP and HTTPS requests.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProxyConfig {
    /// Proxy URL, e.g. `http://proxy.example.com:3128`.
    pub url: String,
    /// Comma-separated hosts reached without the proxy. An entry matches
    /// the host and its subdomains; a leading `.` is ignored.
    /// Example: `"internal.example.com, .svc.cluster.local"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_proxy: Option<String>,
}

pub(super) fn one_or_many<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<PathBuf>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged, expecting = "a path or a list of paths")]
    enum OneOrMany {
        One(PathBuf),
        Many(Vec<PathBuf>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(p) => vec![p],
        OneOrMany::Many(v) => v,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ca_takes_one_path_or_a_list() {
        let one: TlsConfig = serde_yaml::from_str("ca: /etc/vigil/ca.pem").unwrap();
        assert_eq!(one.ca, vec![PathBuf::from("/etc/vigil/ca.pem")]);
        let many: TlsConfig =
            serde_yaml::from_str("ca: [/etc/vigil/ca.pem, /etc/vigil/proxy-ca.pem]").unwrap();
        assert_eq!(many.ca.len(), 2);
        let err = serde_yaml::from_str::<TlsConfig>("ca: {path: /x}")
            .unwrap_err()
            .to_string();
        assert!(err.contains("a path or a list of paths"), "{err}");
    }

    #[test]
    fn unknown_keys_are_errors() {
        let err = serde_yaml::from_str::<TlsConfig>("ca-cert: /etc/vigil/ca.pem")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field `ca-cert`"), "{err}");
        let err = serde_yaml::from_str::<ProxyConfig>("url: http://p:3128\nca: /x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field `ca`"), "{err}");
    }

    #[test]
    fn proxy_needs_a_url() {
        let err = serde_yaml::from_str::<ProxyConfig>("no-proxy: example.com")
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing field `url`"), "{err}");
    }
}
