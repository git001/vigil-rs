// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::extract::ConnectInfo;
use axum::http::StatusCode;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use vigil_types::identity::IdentityAccess;

use crate::identity::IdentityStore;
use crate::server::UnixPeerInfo;

use super::AppState;

// ---------------------------------------------------------------------------
// Access resolution
// ---------------------------------------------------------------------------

/// Resolves the caller's access level from request parts.
///
/// Resolution order:
/// 1. **Bootstrap** — if the identity store is empty, grant `Admin` so the
///    operator can add their first identity.
/// 2. **HTTP Basic Auth** — `Authorization: Basic <base64(user:pass)>` header.
/// 3. **Unix peer UID** — for connections over the Unix socket, match the
///    calling process's UID against local identities.
/// 4. **Fallback** — `Open` (access only to endpoints that require no auth).
async fn resolve_access(
    store: &IdentityStore,
    parts: &axum::http::request::Parts,
) -> IdentityAccess {
    // 1. HTTP Basic Auth
    if let Some(auth_hdr) = parts.headers.get(axum::http::header::AUTHORIZATION) {
        if let Ok(auth_str) = auth_hdr.to_str() {
            if let Some(encoded) = auth_str.strip_prefix("Basic ") {
                if let Ok(decoded) = B64.decode(encoded.trim()) {
                    if let Ok(cred) = std::str::from_utf8(&decoded) {
                        if let Some((user, pass)) = cred.split_once(':') {
                            if let Some(level) = store.basic_access(user, pass).await {
                                return level;
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Unix socket peer UID
    if let Some(ConnectInfo(peer)) = parts.extensions.get::<ConnectInfo<UnixPeerInfo>>() {
        if let Some(uid) = peer.uid {
            if let Some(level) = store.local_access(uid).await {
                return level;
            }
        }
    }

    // 3. Fallback
    IdentityAccess::Open
}

// ---------------------------------------------------------------------------
// Caller extractor
// ---------------------------------------------------------------------------

/// Axum extractor that resolves the caller's access level for the current
/// request. Handlers call `caller.require(level)?` to enforce a minimum
/// access level.
pub struct Caller(pub IdentityAccess);

impl Caller {
    /// Return `403 Forbidden` if this caller's access level is below `needed`.
    pub fn require(self, needed: IdentityAccess) -> Result<(), (StatusCode, &'static str)> {
        if self.0 >= needed {
            Ok(())
        } else {
            Err((StatusCode::FORBIDDEN, "forbidden"))
        }
    }
}

impl axum::extract::FromRequestParts<AppState> for Caller {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let access = if state.identity_store.is_empty().await {
            IdentityAccess::Admin
        } else {
            resolve_access(&state.identity_store, parts).await
        };
        Ok(Caller(access))
    }
}
