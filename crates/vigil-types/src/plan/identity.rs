// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `identities:` in layer files.
//!
//! ```yaml
//! identities:
//!   ops-admin:
//!     access: admin
//!     local: {}
//!   log-relay:
//!     access: read
//!     basic:
//!       password-hash-file: /run/secrets/vigil/log-relay.hash
//!   monitoring:
//!     access: metrics
//!     tls:
//!       ca: /etc/vigil/ca/monitoring-ca.pem
//! ```
//!
//! A later layer replaces a whole identity; there is no `override`, as
//! merging authentication methods would be dangerous.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::identity::{IdentityAccess, LocalIdentity};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct IdentityConfig {
    /// Label (file name without extension) of the layer that defines it.
    #[serde(skip)]
    pub layer: String,
    pub access: IdentityAccess,
    /// Unix-socket callers; without `user-id`, every UID that can reach the
    /// socket (vigild's own and root, see the prefix directory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic: Option<BasicConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<IdentityTls>,
}

/// HTTP Basic auth: the SHA-512 crypt hash inline or from a file (e.g. a
/// mounted Kubernetes Secret); exactly one of the two.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BasicConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    /// Absolute path of a file with the hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash_file: Option<PathBuf>,
}

/// TLS client certificates signed by one of these CAs. Only `ca`: the
/// other keys of the `tls:` block of checks and alerts (`insecure`, `cert`,
/// `key`) make no sense here, and `insecure` would bypass authentication.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct IdentityTls {
    /// PEM files with the CA certificates: one absolute path or a list.
    #[serde(deserialize_with = "super::tls::one_or_many")]
    pub ca: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Result<IdentityConfig, String> {
        serde_yaml::from_str(yaml).map_err(|e| e.to_string())
    }

    #[test]
    fn tls_takes_one_ca_or_a_list() {
        let one = parse("access: metrics\ntls:\n  ca: /etc/vigil/ca/monitoring-ca.pem\n").unwrap();
        assert_eq!(one.tls.unwrap().ca.len(), 1);
        let many =
            parse("access: metrics\ntls:\n  ca: [/etc/vigil/ca/a.pem, /etc/vigil/ca/b.pem]\n")
                .unwrap();
        assert_eq!(many.tls.unwrap().ca.len(), 2);
    }

    #[test]
    fn tls_accepts_only_ca() {
        for key in ["insecure: true", "cert: /run/secrets/vigil/client.crt"] {
            let err = parse(&format!(
                "access: admin\ntls:\n  ca: /etc/vigil/ca/a.pem\n  {key}\n"
            ))
            .unwrap_err();
            assert!(err.contains("unknown field"), "{err}");
        }
        let err = parse("access: admin\ntls: {}\n").unwrap_err();
        assert!(err.contains("missing field `ca`"), "{err}");
    }

    #[test]
    fn unknown_keys_are_errors() {
        for yaml in [
            "access: admin\nlocal:\n  uid: 1000\n",
            "access: read\nbasic:\n  password: MY-pw1!\n",
            "access: read\npassword-hash: x\n",
        ] {
            let err = parse(yaml).unwrap_err();
            assert!(err.contains("unknown field"), "{yaml}: {err}");
        }
        let ok = parse("access: admin\nlocal:\n  user-id: 1000\n").unwrap();
        assert_eq!(ok.local.unwrap().user_id, Some(1000));
    }
}
