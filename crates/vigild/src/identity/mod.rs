// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::collections::HashMap;
use std::sync::Arc;

use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::WebPkiClientVerifier;
use sha_crypt::sha512_check;
use tokio::sync::RwLock;
use tracing::warn;
use vigil_types::identity::{Identity, IdentityAccess, IdentitySource, IdentitySpec};

mod layer;

pub use layer::{LoadedIdentities, load_layer_identities};

// ---------------------------------------------------------------------------
// In-memory identity store
// ---------------------------------------------------------------------------

/// All identities: those from the layer files ([`IdentitySource::Layer`],
/// replaced as a whole on every load) and those added over the API
/// ([`IdentitySource::Api`]). Layer identities are read-only over the API.
#[derive(Default)]
pub struct IdentityStore {
    inner: RwLock<HashMap<String, Identity>>,
}

impl IdentityStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A store holding the layer identities of the first load.
    pub fn with_layer(identities: Vec<Identity>) -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(
                identities
                    .into_iter()
                    .map(|id| (id.name.clone(), id))
                    .collect(),
            ),
        })
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

    /// No identity was added over the API. Then root and vigild's own UID
    /// keep the Unix-socket bootstrap admin, even with layer identities.
    pub async fn api_is_empty(&self) -> bool {
        !self
            .inner
            .read()
            .await
            .values()
            .any(|id| id.source == IdentitySource::Api)
    }

    /// Replace all layer identities (startup, replan). An API identity of
    /// the same name is replaced too: the layer files win.
    pub async fn set_layer(&self, identities: Vec<Identity>) {
        let mut guard = self.inner.write().await;
        guard.retain(|_, id| id.source == IdentitySource::Api);
        for id in identities {
            if guard.contains_key(&id.name) {
                warn!(
                    identity = %id.name,
                    layer = id.layer.as_deref().unwrap_or_default(),
                    "identity added over the API is replaced by the one in the layer file"
                );
            }
            guard.insert(id.name.clone(), id);
        }
    }

    /// Add or update an identity over the API. Layer identities cannot be
    /// changed this way.
    pub async fn add(&self, name: String, spec: IdentitySpec) -> anyhow::Result<()> {
        let mut guard = self.inner.write().await;
        if let Some(existing) = guard.get(&name) {
            ensure_not_layer(existing)?;
        }
        let identity = Identity {
            name: name.clone(),
            access: spec.access,
            local: spec.local,
            basic: spec.basic,
            tls: spec.tls,
            source: IdentitySource::Api,
            layer: None,
        };
        guard.insert(name, identity);
        Ok(())
    }

    /// Remove identities added over the API; returns the names removed.
    /// Naming a layer identity fails the whole request (nothing removed).
    pub async fn remove(&self, names: &[String]) -> anyhow::Result<Vec<String>> {
        let mut guard = self.inner.write().await;
        for name in names {
            if let Some(existing) = guard.get(name) {
                ensure_not_layer(existing)?;
            }
        }
        Ok(names
            .iter()
            .filter(|name| guard.remove(*name).is_some())
            .cloned()
            .collect())
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

fn ensure_not_layer(id: &Identity) -> anyhow::Result<()> {
    anyhow::ensure!(
        id.source != IdentitySource::Layer,
        "identity {:?} is defined in layer {:?}; change it there",
        id.name,
        id.layer.as_deref().unwrap_or_default()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Certificate verification helper
// ---------------------------------------------------------------------------

/// Returns `true` if `cert_der` is a valid end-entity certificate that was
/// signed by one of the CA certificates in `ca_pem` (PEM, may be a chain).
pub(crate) fn verify_cert_against_ca(cert_der: &[u8], ca_pem: &str) -> bool {
    let Ok(cas) = vigil_tls::certs_from_pem("ca-cert", ca_pem.as_bytes()) else {
        return false;
    };
    let mut roots = RootCertStore::empty();
    for ca in cas {
        if roots.add(ca).is_err() {
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
