// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use sha_crypt::sha512_check;
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
