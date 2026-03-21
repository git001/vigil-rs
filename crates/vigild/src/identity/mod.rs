// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::collections::HashMap;
use std::sync::Arc;

use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::WebPkiClientVerifier;
use sha_crypt::sha512_check;
use tokio::sync::RwLock;
use vigil_types::identity::{Identity, IdentityAccess, IdentitySpec};

// ---------------------------------------------------------------------------
// In-memory identity store
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct IdentityStore {
    inner: RwLock<HashMap<String, Identity>>,
}

impl IdentityStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn list(&self, names: &[String]) -> Vec<Identity> {
        self.inner
            .read()
            .await
            .values()
            .filter(|id| names.is_empty() || names.contains(&id.name))
            .cloned()
            .collect()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }

    pub async fn add(&self, name: String, spec: IdentitySpec) {
        let identity = Identity {
            name: name.clone(),
            access: spec.access,
            local: spec.local,
            basic: spec.basic,
            tls: spec.tls,
        };
        self.inner.write().await.insert(name, identity);
    }

    pub async fn remove(&self, names: &[String]) -> Vec<String> {
        let mut guard = self.inner.write().await;
        let mut removed = Vec::new();
        for name in names {
            if guard.remove(name).is_some() {
                removed.push(name.clone());
            }
        }
        removed
    }

    /// Return the effective access level for an incoming Unix-socket connection
    /// with the given UID, or `None` if no matching identity is found.
    pub async fn local_access(&self, uid: u32) -> Option<IdentityAccess> {
        self.inner
            .read()
            .await
            .values()
            .filter_map(|id| {
                let local = id.local.as_ref()?;
                // match if user_id is unset (any) or matches exactly
                if local.user_id.is_none() || local.user_id == Some(uid) {
                    Some(id.access)
                } else {
                    None
                }
            })
            // IdentityAccess is Ord — take the most permissive level
            .max()
    }

    /// Verify a TLS client certificate against all registered TLS identities
    /// and return the most-permissive matching access level, or `None` if no
    /// identity's CA signed the certificate.
    pub async fn tls_access(&self, cert_der: &[u8]) -> Option<IdentityAccess> {
        let guard = self.inner.read().await;
        guard
            .values()
            .filter_map(|id| {
                let tls = id.tls.as_ref()?;
                if verify_cert_against_ca(cert_der, &tls.ca_cert) {
                    Some(id.access)
                } else {
                    None
                }
            })
            .max()
    }

    /// Verify an HTTP Basic Auth credential and return the access level,
    /// or `None` if the username is unknown or the password does not match.
    pub async fn basic_access(&self, username: &str, password: &str) -> Option<IdentityAccess> {
        let guard = self.inner.read().await;
        let identity = guard.get(username)?;
        let basic = identity.basic.as_ref()?;
        sha512_check(password, &basic.password_hash)
            .ok()
            .map(|_| identity.access)
    }
}

// ---------------------------------------------------------------------------
// Certificate verification helper
// ---------------------------------------------------------------------------

/// Returns `true` if `cert_der` is a valid end-entity certificate that was
/// signed by one of the CA certificates in `ca_pem` (PEM, may be a chain).
pub(crate) fn verify_cert_against_ca(cert_der: &[u8], ca_pem: &str) -> bool {
    let Ok(ca_ders) = rustls_pemfile::certs(&mut ca_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
    else {
        return false;
    };
    if ca_ders.is_empty() {
        return false;
    }
    let mut roots = RootCertStore::empty();
    for ca_der in ca_ders {
        if roots.add(ca_der).is_err() {
            return false;
        }
    }
    let Ok(verifier) = WebPkiClientVerifier::builder(Arc::new(roots)).build() else {
        return false;
    };
    let cert = CertificateDer::from(cert_der.to_vec());
    verifier
        .verify_client_cert(&cert, &[], UnixTime::now())
        .is_ok()
}

#[cfg(test)]
mod tests;
