// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

// ---------------------------------------------------------------------------
// Identity types
// ---------------------------------------------------------------------------

/// Access levels ordered from least to most privileged.
///
/// | Level     | Description |
/// |-----------|-------------|
/// | `open`    | No authentication required — health and system-info endpoints. |
/// | `metrics` | Access to `GET /v1/metrics` only. |
/// | `read`    | Most `GET` endpoints. |
/// | `write`   | Read + service/check control (start, stop, restart, trigger). |
/// | `admin`   | Full access including identity management. |
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum IdentityAccess {
    Open,
    Metrics,
    Read,
    Write,
    Admin,
}

/// Matches a connection coming in over the Unix socket by the caller's UID.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct LocalIdentity {
    /// Restrict to this UID. Omit to allow any local user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u32>,
}

/// Authenticates via HTTP Basic Auth.
///
/// The password is verified against a SHA-512-crypt hash (format: `$6$salt$hash`).
/// Generate a hash with: `openssl passwd -6`
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct BasicIdentity {
    /// SHA-512-crypt password hash (`$6$...`).
    pub password_hash: String,
}

/// Matches a TLS connection whose client cert was signed by the given CA.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct TlsIdentity {
    /// PEM-encoded CA certificate used to verify the client cert.
    pub ca_cert: String,
}

/// A named principal with a declared access level and optional auth info.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct Identity {
    pub name: String,
    pub access: IdentityAccess,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basic: Option<BasicIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsIdentity>,
}

// ---------------------------------------------------------------------------
// API request/response types
// ---------------------------------------------------------------------------

/// Body for `POST /v1/identities` (add or update identities).
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AddIdentitiesRequest {
    pub identities: HashMap<String, IdentitySpec>,
}

/// Spec used when adding/updating an identity (no `name` field — that's the key).
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub struct IdentitySpec {
    pub access: IdentityAccess,
    #[serde(default)]
    pub local: Option<LocalIdentity>,
    #[serde(default)]
    pub basic: Option<BasicIdentity>,
    #[serde(default)]
    pub tls: Option<TlsIdentity>,
}

/// Body for `DELETE /v1/identities`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RemoveIdentitiesRequest {
    pub identities: Vec<String>,
}
