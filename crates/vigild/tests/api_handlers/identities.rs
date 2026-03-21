use axum::http::{Method, StatusCode};
use serde_json::Value;

use super::{TestApp, body_json};

use vigild::testutil::init_crypto;

/// Generate a CA cert (PEM) + a client cert signed by that CA (DER).
fn gen_ca_and_client_cert() -> (String, Vec<u8>) {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(vec!["test-ca".to_string()]).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let ca_pem = ca_cert.pem();
    let client_key = KeyPair::generate().unwrap();
    let client_params = CertificateParams::new(vec!["test-client".to_string()]).unwrap();
    let client_cert = client_params
        .signed_by(&client_key, &ca_cert, &ca_key)
        .unwrap();
    (ca_pem, client_cert.der().to_vec())
}

// ---------------------------------------------------------------------------
// identities
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_identities_returns_empty_when_no_identities() {
    let app = TestApp::new().await;
    let resp = app.get("/v1/identities").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["result"], Value::Array(vec![]));
    app.shutdown().await;
}

#[tokio::test]
async fn add_then_list_identity() {
    let app = TestApp::new().await;

    // Bootstrap: store empty → Admin. Add alice with admin + local (any UID)
    // so that subsequent requests using ConnectInfo can still authenticate.
    let resp = app
        .request(
            Method::POST,
            "/v1/identities",
            Some(serde_json::json!({
                "identities": {
                    "alice": { "access": "admin", "local": {} }
                }
            })),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Store is no longer empty — use local UID auth for the list request.
    let resp = app.request_auth(Method::GET, "/v1/identities", None).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "expected 200 on GET /v1/identities with auth"
    );
    let body = body_json(resp).await;
    let names: Vec<&str> = body["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"alice"), "alice not found in {names:?}");
    app.shutdown().await;
}

#[tokio::test]
async fn remove_identity() {
    let app = TestApp::new().await;

    // Bootstrap: add bob with admin + local (any UID) so we can re-auth.
    app.request(
        Method::POST,
        "/v1/identities",
        Some(serde_json::json!({
            "identities": { "bob": { "access": "admin", "local": {} } }
        })),
    )
    .await;

    // Remove bob — use local UID auth since bootstrap mode has ended.
    let resp = app
        .request_auth(
            Method::DELETE,
            "/v1/identities",
            Some(serde_json::json!({ "identities": ["bob"] })),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["result"], serde_json::json!(["bob"]));

    app.shutdown().await;
}

#[tokio::test]
async fn remove_nonexistent_identity_returns_empty_list() {
    let app = TestApp::new().await;
    let resp = app
        .request(
            Method::DELETE,
            "/v1/identities",
            Some(serde_json::json!({ "identities": ["nobody"] })),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["result"], serde_json::json!([]));
    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// Auth: once an identity is added, unauthenticated callers lose admin access
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_open_caller_forbidden_after_identity_added() {
    let app = TestApp::new().await;

    // Add an identity — bootstrap mode ends, open callers get Open access only
    app.request(
        Method::POST,
        "/v1/identities",
        Some(serde_json::json!({
            "identities": {
                "operator": { "access": "admin", "local": {} }
            }
        })),
    )
    .await;

    // Now an unauthenticated GET /v1/identities (requires Admin) should be 403
    let resp = app.get("/v1/identities").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    app.shutdown().await;
}

#[tokio::test]
async fn auth_read_endpoint_forbidden_without_credentials() {
    let app = TestApp::new().await;

    // Add an identity to end bootstrap mode
    app.request(
        Method::POST,
        "/v1/identities",
        Some(serde_json::json!({
            "identities": { "op": { "access": "admin", "local": {} } }
        })),
    )
    .await;

    // /v1/services requires Read — unauthenticated caller has Open → forbidden
    let resp = app.get("/v1/services").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// Auth: Basic Auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_basic_auth_correct_password_grants_access() {
    use sha_crypt::{Sha512Params, sha512_simple};

    let app = TestApp::new().await;
    let params = Sha512Params::new(1000).unwrap();
    let hash = sha512_simple("secret", &params).unwrap();

    // Bootstrap: add basic identity with Read access
    app.request(
        Method::POST,
        "/v1/identities",
        Some(serde_json::json!({
            "identities": {
                "deploy": { "access": "read", "basic": { "password-hash": hash } }
            }
        })),
    )
    .await;

    // Correct credentials → Read → GET /v1/services returns 200
    let resp = app
        .request_basic_auth(Method::GET, "/v1/services", "deploy", "secret", None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

#[tokio::test]
async fn auth_basic_auth_wrong_password_returns_forbidden() {
    use sha_crypt::{Sha512Params, sha512_simple};

    let app = TestApp::new().await;
    let params = Sha512Params::new(1000).unwrap();
    let hash = sha512_simple("correct", &params).unwrap();

    app.request(
        Method::POST,
        "/v1/identities",
        Some(serde_json::json!({
            "identities": {
                "deploy": { "access": "read", "basic": { "password-hash": hash } }
            }
        })),
    )
    .await;

    let resp = app
        .request_basic_auth(Method::GET, "/v1/services", "deploy", "wrongpass", None)
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// Auth: mTLS client certificate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_mtls_valid_cert_grants_access() {
    init_crypto();
    let app = TestApp::new().await;
    let (ca_pem, client_der) = gen_ca_and_client_cert();

    // Bootstrap: add TLS identity — any cert signed by this CA gets Read
    app.request(
        Method::POST,
        "/v1/identities",
        Some(serde_json::json!({
            "identities": {
                "ops": { "access": "read", "tls": { "ca-cert": ca_pem } }
            }
        })),
    )
    .await;

    // Inject the client cert DER as the TLS peer cert → Read → 200
    let resp = app
        .request_mtls(Method::GET, "/v1/services", client_der, None)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    app.shutdown().await;
}

#[tokio::test]
async fn auth_mtls_cert_from_wrong_ca_returns_forbidden() {
    init_crypto();
    let app = TestApp::new().await;
    let (ca_pem, _) = gen_ca_and_client_cert();
    let (_, other_client_der) = gen_ca_and_client_cert(); // signed by a different CA

    app.request(
        Method::POST,
        "/v1/identities",
        Some(serde_json::json!({
            "identities": {
                "ops": { "access": "read", "tls": { "ca-cert": ca_pem } }
            }
        })),
    )
    .await;

    // Cert not signed by the registered CA → no match → Open → 403
    let resp = app
        .request_mtls(Method::GET, "/v1/services", other_client_der, None)
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    app.shutdown().await;
}
