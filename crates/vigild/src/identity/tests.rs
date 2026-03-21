// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use sha_crypt::{Sha512Params, sha512_simple};
use vigil_types::identity::{BasicIdentity, IdentityAccess, IdentitySpec, LocalIdentity};

use super::{IdentityStore, verify_cert_against_ca};

fn spec(access: IdentityAccess) -> IdentitySpec {
    IdentitySpec {
        access,
        local: None,
        basic: None,
        tls: None,
    }
}

fn local_spec(access: IdentityAccess, user_id: Option<u32>) -> IdentitySpec {
    IdentitySpec {
        access,
        local: Some(LocalIdentity { user_id }),
        basic: None,
        tls: None,
    }
}

fn basic_spec(access: IdentityAccess, password: &str) -> IdentitySpec {
    let params = Sha512Params::new(1000).unwrap();
    let hash = sha512_simple(password, &params).unwrap();
    IdentitySpec {
        access,
        local: None,
        basic: Some(BasicIdentity {
            password_hash: hash,
        }),
        tls: None,
    }
}

#[tokio::test]
async fn add_and_list_all() {
    let store = IdentityStore::new();
    store.add("alice".into(), spec(IdentityAccess::Read)).await;
    store.add("bob".into(), spec(IdentityAccess::Write)).await;
    let mut names: Vec<String> = store.list(&[]).await.into_iter().map(|i| i.name).collect();
    names.sort();
    assert_eq!(names, vec!["alice", "bob"]);
}

#[tokio::test]
async fn list_with_filter() {
    let store = IdentityStore::new();
    store.add("alice".into(), spec(IdentityAccess::Read)).await;
    store.add("bob".into(), spec(IdentityAccess::Admin)).await;
    let found = store.list(&["alice".to_string()]).await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "alice");
}

#[tokio::test]
async fn list_filter_unknown_returns_empty() {
    let store = IdentityStore::new();
    store.add("alice".into(), spec(IdentityAccess::Read)).await;
    let found = store.list(&["nobody".to_string()]).await;
    assert!(found.is_empty());
}

#[tokio::test]
async fn is_empty_after_creation() {
    let store = IdentityStore::new();
    assert!(store.is_empty().await);
    store.add("alice".into(), spec(IdentityAccess::Read)).await;
    assert!(!store.is_empty().await);
}

#[tokio::test]
async fn remove_existing() {
    let store = IdentityStore::new();
    store.add("alice".into(), spec(IdentityAccess::Read)).await;
    let removed = store.remove(&["alice".to_string()]).await;
    assert_eq!(removed, vec!["alice"]);
    assert!(store.is_empty().await);
}

#[tokio::test]
async fn remove_nonexistent_returns_empty_list() {
    let store = IdentityStore::new();
    let removed = store.remove(&["ghost".to_string()]).await;
    assert!(removed.is_empty());
}

#[tokio::test]
async fn remove_mixed_existing_and_missing() {
    let store = IdentityStore::new();
    store.add("alice".into(), spec(IdentityAccess::Read)).await;
    let removed = store
        .remove(&["alice".to_string(), "ghost".to_string()])
        .await;
    assert_eq!(removed, vec!["alice"]);
}

#[tokio::test]
async fn local_access_any_uid() {
    let store = IdentityStore::new();
    store
        .add("local".into(), local_spec(IdentityAccess::Write, None))
        .await;
    assert_eq!(store.local_access(0).await, Some(IdentityAccess::Write));
    assert_eq!(store.local_access(1000).await, Some(IdentityAccess::Write));
    assert_eq!(store.local_access(99999).await, Some(IdentityAccess::Write));
}

#[tokio::test]
async fn local_access_specific_uid_match_and_miss() {
    let store = IdentityStore::new();
    store
        .add("uid42".into(), local_spec(IdentityAccess::Admin, Some(42)))
        .await;
    assert_eq!(store.local_access(42).await, Some(IdentityAccess::Admin));
    assert_eq!(store.local_access(99).await, None);
}

#[tokio::test]
async fn local_access_returns_most_permissive() {
    let store = IdentityStore::new();
    store
        .add("read-all".into(), local_spec(IdentityAccess::Read, None))
        .await;
    store
        .add(
            "admin-uid0".into(),
            local_spec(IdentityAccess::Admin, Some(0)),
        )
        .await;
    // uid 0 matches both — should get Admin (max)
    assert_eq!(store.local_access(0).await, Some(IdentityAccess::Admin));
    // uid 1 matches only read-all
    assert_eq!(store.local_access(1).await, Some(IdentityAccess::Read));
}

#[tokio::test]
async fn local_access_empty_store_returns_none() {
    let store = IdentityStore::new();
    assert!(store.local_access(0).await.is_none());
}

#[tokio::test]
async fn local_access_no_local_identity_returns_none() {
    let store = IdentityStore::new();
    // identity with no local config
    store
        .add("apionly".into(), spec(IdentityAccess::Read))
        .await;
    assert!(store.local_access(1000).await.is_none());
}

#[tokio::test]
async fn basic_access_correct_password() {
    let store = IdentityStore::new();
    store
        .add("carol".into(), basic_spec(IdentityAccess::Write, "s3cr3t"))
        .await;
    assert_eq!(
        store.basic_access("carol", "s3cr3t").await,
        Some(IdentityAccess::Write)
    );
}

#[tokio::test]
async fn basic_access_wrong_password() {
    let store = IdentityStore::new();
    store
        .add("carol".into(), basic_spec(IdentityAccess::Write, "s3cr3t"))
        .await;
    assert_eq!(store.basic_access("carol", "wrong").await, None);
}

#[tokio::test]
async fn basic_access_unknown_user() {
    let store = IdentityStore::new();
    assert!(store.basic_access("nobody", "pass").await.is_none());
}

#[tokio::test]
async fn basic_access_no_basic_config_returns_none() {
    let store = IdentityStore::new();
    store.add("alice".into(), spec(IdentityAccess::Read)).await;
    assert!(store.basic_access("alice", "anything").await.is_none());
}

#[tokio::test]
async fn add_overwrites_existing_identity() {
    let store = IdentityStore::new();
    store.add("alice".into(), spec(IdentityAccess::Read)).await;
    store.add("alice".into(), spec(IdentityAccess::Admin)).await;
    let ids = store.list(&[]).await;
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].access, IdentityAccess::Admin);
}

// ------------------------------------------------------------------
// verify_cert_against_ca
// ------------------------------------------------------------------

use crate::testutil::init_crypto;

/// Generate a CA cert + a client cert signed by that CA.
/// Returns `(ca_pem, client_cert_der)`.
fn gen_ca_and_client_cert() -> (String, Vec<u8>) {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(vec!["test-ca".to_string()]).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let ca_pem = ca_cert.pem();

    let client_key = KeyPair::generate().unwrap();
    let client_params = CertificateParams::new(vec!["test-client".to_string()]).unwrap();
    let client_cert = client_params.signed_by(&client_key, &ca_cert, &ca_key).unwrap();
    (ca_pem, client_cert.der().to_vec())
}

#[test]
fn verify_cert_against_ca_valid_cert_and_ca() {
    init_crypto();
    let (ca_pem, client_der) = gen_ca_and_client_cert();
    assert!(verify_cert_against_ca(&client_der, &ca_pem));
}

#[test]
fn verify_cert_against_ca_wrong_ca_returns_false() {
    init_crypto();
    let (_, client_der) = gen_ca_and_client_cert();
    let (other_ca_pem, _) = gen_ca_and_client_cert(); // different CA
    assert!(!verify_cert_against_ca(&client_der, &other_ca_pem));
}

#[test]
fn verify_cert_against_ca_empty_ca_pem_returns_false() {
    init_crypto();
    let (_, client_der) = gen_ca_and_client_cert();
    assert!(!verify_cert_against_ca(&client_der, ""));
}

#[test]
fn verify_cert_against_ca_invalid_cert_der_returns_false() {
    init_crypto();
    let (ca_pem, _) = gen_ca_and_client_cert();
    assert!(!verify_cert_against_ca(b"not a cert", &ca_pem));
}

// ------------------------------------------------------------------
// tls_access
// ------------------------------------------------------------------

fn tls_spec(access: IdentityAccess, ca_pem: &str) -> IdentitySpec {
    use vigil_types::identity::TlsIdentity;
    IdentitySpec {
        access,
        local: None,
        basic: None,
        tls: Some(TlsIdentity {
            ca_cert: ca_pem.to_string(),
        }),
    }
}

#[tokio::test]
async fn tls_access_matching_ca_returns_access_level() {
    init_crypto();
    let (ca_pem, client_der) = gen_ca_and_client_cert();
    let store = IdentityStore::new();
    store
        .add("ops".into(), tls_spec(IdentityAccess::Write, &ca_pem))
        .await;
    assert_eq!(
        store.tls_access(&client_der).await,
        Some(IdentityAccess::Write)
    );
}

#[tokio::test]
async fn tls_access_wrong_ca_returns_none() {
    init_crypto();
    let (_, client_der) = gen_ca_and_client_cert();
    let (other_ca_pem, _) = gen_ca_and_client_cert();
    let store = IdentityStore::new();
    store
        .add("ops".into(), tls_spec(IdentityAccess::Admin, &other_ca_pem))
        .await;
    assert_eq!(store.tls_access(&client_der).await, None);
}

#[tokio::test]
async fn tls_access_returns_most_permissive_when_multiple_identities_match() {
    init_crypto();
    let (ca_pem, client_der) = gen_ca_and_client_cert();
    let store = IdentityStore::new();
    store
        .add("read-svc".into(), tls_spec(IdentityAccess::Read, &ca_pem))
        .await;
    store
        .add("admin-svc".into(), tls_spec(IdentityAccess::Admin, &ca_pem))
        .await;
    assert_eq!(
        store.tls_access(&client_der).await,
        Some(IdentityAccess::Admin)
    );
}

#[tokio::test]
async fn tls_access_empty_store_returns_none() {
    init_crypto();
    let (_, client_der) = gen_ca_and_client_cert();
    let store = IdentityStore::new();
    assert_eq!(store.tls_access(&client_der).await, None);
}

#[tokio::test]
async fn tls_access_identity_without_tls_field_not_matched() {
    init_crypto();
    let (ca_pem, client_der) = gen_ca_and_client_cert();
    let _ = ca_pem; // CA registered only for basic, not tls
    let store = IdentityStore::new();
    store
        .add("basic-only".into(), spec(IdentityAccess::Admin))
        .await;
    assert_eq!(store.tls_access(&client_der).await, None);
}
