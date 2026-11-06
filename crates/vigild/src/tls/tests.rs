// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use super::*;
use std::io::Write as _;
use tempfile::NamedTempFile;

use crate::testutil::init_crypto;

/// Encode a private key DER as a PKCS8 PEM block.
fn key_der_to_pem(key_der: &[u8]) -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let b64 = STANDARD.encode(key_der);
    let lines: String = b64
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    format!("-----BEGIN PRIVATE KEY-----\n{lines}\n-----END PRIVATE KEY-----\n")
}

/// Generate a CA cert and a client cert signed by that CA.
/// Returns `(ca_cert_pem, client_cert_der, client_key_der)`.
fn gen_ca_and_client() -> (String, Vec<u8>, Vec<u8>) {
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
    let client_der = client_cert.der().to_vec();
    let client_key_der = client_key.serialize_der();

    (ca_pem, client_der, client_key_der)
}

// -----------------------------------------------------------------------
// generate_self_signed / acceptor_from_der
// -----------------------------------------------------------------------

#[test]
fn generate_self_signed_produces_valid_cert() {
    init_crypto();
    let (certs, key) = generate_self_signed(&["localhost"]).unwrap();
    assert!(!certs.is_empty());
    assert!(!key.secret_der().is_empty());
    // Should also build a valid acceptor.
    acceptor_from_der(certs, key).unwrap();
}

// -----------------------------------------------------------------------
// acceptor_from_der_mtls
// -----------------------------------------------------------------------

#[test]
fn acceptor_from_der_mtls_builds_successfully() {
    init_crypto();
    let (ca_pem, _client_der, _) = gen_ca_and_client();
    let (cert_ders, key_der) = generate_self_signed(&["localhost"]).unwrap();

    let cas = vigil_tls::certs_from_pem("ca", ca_pem.as_bytes()).unwrap();
    acceptor_from_der_mtls(cert_ders, key_der, cas).unwrap();
}

#[test]
fn acceptor_from_der_mtls_empty_ca_list_errors() {
    init_crypto();
    let (cert_ders, key_der) = generate_self_signed(&["localhost"]).unwrap();
    // Empty CA list — WebPkiClientVerifier should reject this.
    let result = acceptor_from_der_mtls(cert_ders, key_der, vec![]);
    assert!(result.is_err());
}

// -----------------------------------------------------------------------
// load_or_generate
// -----------------------------------------------------------------------

#[test]
fn load_or_generate_auto_generates_when_paths_none() {
    init_crypto();
    let acc = load_or_generate(None, None, "localhost", None).unwrap();
    // Acceptor returned — just verify we got something without panicking.
    drop(acc);
}

#[test]
fn load_or_generate_errors_when_only_cert_provided() {
    init_crypto();
    let tmp = NamedTempFile::new().unwrap();
    let result = load_or_generate(Some(tmp.path()), None, "localhost", None);
    assert!(result.is_err());
    let msg = result.err().unwrap().to_string();
    assert!(msg.contains("both") || msg.contains("omitted"));
}

#[test]
fn load_or_generate_from_pem_files() {
    init_crypto();
    let (cert_ders, key_der) = generate_self_signed(&["localhost"]).unwrap();
    let cert_pem = cert_to_pem(&cert_ders[0]);
    let key_pem = key_der_to_pem(key_der.secret_der());

    let mut cert_file = NamedTempFile::new().unwrap();
    cert_file.write_all(cert_pem.as_bytes()).unwrap();
    let mut key_file = NamedTempFile::new().unwrap();
    key_file.write_all(key_pem.as_bytes()).unwrap();

    load_or_generate(
        Some(cert_file.path()),
        Some(key_file.path()),
        "localhost",
        None,
    )
    .unwrap();
}

#[test]
fn load_or_generate_empty_cert_file_errors() {
    init_crypto();
    let cert_file = NamedTempFile::new().unwrap(); // empty
    let mut key_file = NamedTempFile::new().unwrap();
    let (_, key_der) = generate_self_signed(&["localhost"]).unwrap();
    key_file
        .write_all(key_der_to_pem(key_der.secret_der()).as_bytes())
        .unwrap();

    let result = load_or_generate(
        Some(cert_file.path()),
        Some(key_file.path()),
        "localhost",
        None,
    );
    assert!(result.is_err());
    let err = format!("{:#}", result.err().unwrap());
    assert!(
        err.contains("--cert") && err.contains("no PEM certificate found"),
        "{err}"
    );
}

#[test]
fn load_or_generate_with_client_ca_enables_mtls() {
    init_crypto();
    let (ca_pem, _client_der, _) = gen_ca_and_client();

    let mut ca_file = NamedTempFile::new().unwrap();
    ca_file.write_all(ca_pem.as_bytes()).unwrap();

    // Should succeed and build an mTLS-capable acceptor.
    load_or_generate(None, None, "localhost", Some(ca_file.path())).unwrap();
}

#[test]
fn load_or_generate_empty_client_ca_file_errors() {
    init_crypto();
    let empty_ca = NamedTempFile::new().unwrap(); // empty
    let result = load_or_generate(None, None, "localhost", Some(empty_ca.path()));
    assert!(result.is_err());
    let err = format!("{:#}", result.err().unwrap());
    assert!(
        err.contains("--tls-client-ca") && err.contains("no PEM certificate found"),
        "{err}"
    );
}

// -----------------------------------------------------------------------
// build_http_client
// -----------------------------------------------------------------------

use vigil_types::plan::{ProxyConfig, TlsConfig};

fn proxy(url: &str, no_proxy: Option<&str>) -> ProxyConfig {
    ProxyConfig {
        url: url.into(),
        no_proxy: no_proxy.map(Into::into),
    }
}

#[test]
fn build_http_client_without_settings() {
    init_crypto();
    build_http_client(reqwest::Client::builder(), None, None).unwrap();
}

#[test]
fn build_http_client_invalid_proxy_url_errors() {
    init_crypto();
    let err = build_http_client(
        reqwest::Client::builder(),
        None,
        Some(&proxy("not a url at all !!!", None)),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .starts_with("proxy.url \"not a url at all !!!\""),
        "{err}"
    );
}

#[test]
fn build_http_client_with_tls_and_proxy() {
    init_crypto();
    let (ca_pem, _, _) = gen_ca_and_client();
    let mut ca_file = NamedTempFile::new().unwrap();
    ca_file.write_all(ca_pem.as_bytes()).unwrap();
    let tls = TlsConfig {
        ca: vec![ca_file.path().to_owned(), ca_file.path().to_owned()],
        ..Default::default()
    };
    build_http_client(
        reqwest::Client::builder(),
        Some(&tls),
        Some(&proxy(
            "http://proxy.example.com:3128",
            Some("10.0.0.0/8, .svc.cluster.local"),
        )),
    )
    .unwrap();
}

#[test]
fn build_http_client_tls_errors_name_the_setting() {
    init_crypto();
    let relative = TlsConfig {
        ca: vec!["certs/ca.pem".into()],
        ..Default::default()
    };
    let err = build_http_client(reqwest::Client::builder(), Some(&relative), None).unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "tls.ca \"certs/ca.pem\": path must be absolute"
    );

    let (ca_pem, _, _) = gen_ca_and_client();
    let mut cert = NamedTempFile::new().unwrap();
    cert.write_all(ca_pem.as_bytes()).unwrap();
    let cert_only = TlsConfig {
        cert: Some(cert.path().to_owned()),
        ..Default::default()
    };
    let err = build_http_client(reqwest::Client::builder(), Some(&cert_only), None).unwrap_err();
    assert_eq!(format!("{err:#}"), "tls.cert needs a key as well");
}

/// Runs `client.get(url)` and returns the error.
fn get_err(client: &reqwest::Client, url: &str) -> reqwest::Error {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(client.get(url).send()).unwrap_err()
}

#[test]
fn build_http_client_ignores_proxy_env_vars() {
    // vigild's HTTPS_PROXY is not used unless the caller passes it (alerts:
    // their pass-env policy). A request to a closed local port then fails
    // with "connection refused", not with a proxy error.
    init_crypto();
    // Safety: single-threaded test binary, no concurrent env mutation.
    unsafe { std::env::set_var("HTTPS_PROXY", "http://proxy.invalid:3128") };
    unsafe { std::env::set_var("HTTP_PROXY", "http://proxy.invalid:3128") };
    let client = build_http_client(reqwest::Client::builder(), None, None).unwrap();
    unsafe { std::env::remove_var("HTTPS_PROXY") };
    unsafe { std::env::remove_var("HTTP_PROXY") };
    let err = get_err(&client, "http://127.0.0.1:1/");
    assert!(err.is_connect(), "{err:?}");
    assert!(!format!("{err:?}").contains("proxy.invalid"), "{err:?}");
}

#[test]
fn build_http_client_no_proxy_takes_cidr() {
    init_crypto();
    // Through the (unreachable) proxy unless no-proxy matches.
    let via_proxy = build_http_client(
        reqwest::Client::builder(),
        None,
        Some(&proxy("http://127.0.0.1:2", None)),
    )
    .unwrap();
    let direct = build_http_client(
        reqwest::Client::builder(),
        None,
        Some(&proxy("http://127.0.0.1:2", Some("127.0.0.0/8"))),
    )
    .unwrap();
    let err = get_err(&via_proxy, "http://127.0.0.1:1/");
    assert!(format!("{err:?}").contains(":2"), "{err:?}");
    let err = get_err(&direct, "http://127.0.0.1:1/");
    assert!(!format!("{err:?}").contains(":2"), "{err:?}");
}
