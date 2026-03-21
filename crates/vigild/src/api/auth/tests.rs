// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use vigil_types::identity::IdentityAccess;

use super::{Caller, resolve_access};

#[test]
fn require_ok_when_caller_meets_level() {
    assert!(
        Caller(IdentityAccess::Admin)
            .require(IdentityAccess::Read)
            .is_ok()
    );
    assert!(
        Caller(IdentityAccess::Write)
            .require(IdentityAccess::Write)
            .is_ok()
    );
    assert!(
        Caller(IdentityAccess::Read)
            .require(IdentityAccess::Open)
            .is_ok()
    );
}

#[test]
fn require_err_when_caller_below_level() {
    let result = Caller(IdentityAccess::Open).require(IdentityAccess::Read);
    assert!(result.is_err());
    let (status, _) = result.unwrap_err();
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[test]
fn require_err_metrics_below_read() {
    assert!(
        Caller(IdentityAccess::Metrics)
            .require(IdentityAccess::Read)
            .is_err()
    );
}

#[test]
fn require_ok_admin_for_all_levels() {
    for level in [
        IdentityAccess::Open,
        IdentityAccess::Metrics,
        IdentityAccess::Read,
        IdentityAccess::Write,
        IdentityAccess::Admin,
    ] {
        assert!(
            Caller(IdentityAccess::Admin).require(level).is_ok(),
            "Admin should pass require({level:?})"
        );
    }
}

// ------------------------------------------------------------------
// resolve_access — Axum test harness covering all parsing branches
// ------------------------------------------------------------------

fn empty_store() -> std::sync::Arc<crate::identity::IdentityStore> {
    crate::identity::IdentityStore::new()
}

/// Build `Parts` from a request with the given Authorization header value.
fn parts_with_auth(value: &str) -> axum::http::request::Parts {
    let (parts, _) = axum::http::Request::builder()
        .header(axum::http::header::AUTHORIZATION, value)
        .body(())
        .unwrap()
        .into_parts();
    parts
}

fn parts_no_auth() -> axum::http::request::Parts {
    let (parts, _) = axum::http::Request::builder()
        .body(())
        .unwrap()
        .into_parts();
    parts
}

// No Authorization header → Open (falls through to fallback).
#[tokio::test]
async fn no_auth_header_returns_open() {
    let store = empty_store();
    let access = resolve_access(&store, &parts_no_auth()).await;
    assert_eq!(access, IdentityAccess::Open);
}

// Non-Basic scheme (e.g. Bearer) → ignored, Open returned.
#[tokio::test]
async fn bearer_scheme_returns_open() {
    let access = resolve_access(&empty_store(), &parts_with_auth("Bearer some-token")).await;
    assert_eq!(access, IdentityAccess::Open);
}

// "Basic " prefix but invalid base64 → decode fails, falls through → Open.
#[tokio::test]
async fn invalid_base64_returns_open() {
    let access = resolve_access(&empty_store(), &parts_with_auth("Basic not!valid==")).await;
    assert_eq!(access, IdentityAccess::Open);
}

// Valid base64 but decoded string has no ':' → split_once fails → Open.
#[tokio::test]
async fn base64_without_colon_returns_open() {
    let encoded = B64.encode("usernameonly");
    let access = resolve_access(
        &empty_store(),
        &parts_with_auth(&format!("Basic {encoded}")),
    )
    .await;
    assert_eq!(access, IdentityAccess::Open);
}

// Valid "user:pass" format but identity store is empty → no match → Open.
#[tokio::test]
async fn unknown_user_returns_open() {
    let encoded = B64.encode("alice:secret");
    let access = resolve_access(
        &empty_store(),
        &parts_with_auth(&format!("Basic {encoded}")),
    )
    .await;
    assert_eq!(access, IdentityAccess::Open);
}

// Valid "user:pass" with a registered identity and correct password → access granted.
#[tokio::test]
async fn correct_basic_auth_returns_level() {
    use sha_crypt::{Sha512Params, sha512_simple};
    use vigil_types::identity::{BasicIdentity, IdentityAccess, IdentitySpec};

    let store = empty_store();
    let params = Sha512Params::new(1000).unwrap();
    let hash = sha512_simple("hunter2", &params).unwrap();
    store
        .add(
            "bob".to_string(),
            IdentitySpec {
                access: IdentityAccess::Read,
                basic: Some(BasicIdentity {
                    password_hash: hash,
                }),
                local: None,
                tls: None,
            },
        )
        .await;

    let encoded = B64.encode("bob:hunter2");
    let access = resolve_access(&store, &parts_with_auth(&format!("Basic {encoded}"))).await;
    assert_eq!(access, IdentityAccess::Read);
}

// Wrong password for a known user → basic_access returns None → Open.
#[tokio::test]
async fn wrong_password_returns_open() {
    use sha_crypt::{Sha512Params, sha512_simple};
    use vigil_types::identity::{BasicIdentity, IdentityAccess, IdentitySpec};

    let store = empty_store();
    let params = Sha512Params::new(1000).unwrap();
    let hash = sha512_simple("correct", &params).unwrap();
    store
        .add(
            "carol".to_string(),
            IdentitySpec {
                access: IdentityAccess::Admin,
                basic: Some(BasicIdentity {
                    password_hash: hash,
                }),
                local: None,
                tls: None,
            },
        )
        .await;

    let encoded = B64.encode("carol:wrong");
    let access = resolve_access(&store, &parts_with_auth(&format!("Basic {encoded}"))).await;
    assert_eq!(access, IdentityAccess::Open);
}
