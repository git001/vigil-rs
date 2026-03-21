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
    let client_cert = client_params.signed_by(&client_key, &ca_cert, &ca_key).unwrap();
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
    assert!(!key.is_empty());
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

    let ca_der: Vec<Vec<u8>> = rustls_pemfile::certs(&mut ca_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .map(|c| c.to_vec())
        .collect();

    acceptor_from_der_mtls(cert_ders, key_der, ca_der).unwrap();
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
    let key_pem = key_der_to_pem(&key_der);

    let mut cert_file = NamedTempFile::new().unwrap();
    cert_file.write_all(cert_pem.as_bytes()).unwrap();
    let mut key_file = NamedTempFile::new().unwrap();
    key_file.write_all(key_pem.as_bytes()).unwrap();

    load_or_generate(Some(cert_file.path()), Some(key_file.path()), "localhost", None).unwrap();
}

#[test]
fn load_or_generate_empty_cert_file_errors() {
    init_crypto();
    let cert_file = NamedTempFile::new().unwrap(); // empty
    let mut key_file = NamedTempFile::new().unwrap();
    let (_, key_der) = generate_self_signed(&["localhost"]).unwrap();
    key_file
        .write_all(key_der_to_pem(&key_der).as_bytes())
        .unwrap();

    let result = load_or_generate(Some(cert_file.path()), Some(key_file.path()), "localhost", None);
    assert!(result.is_err());
    assert!(result.err().unwrap().to_string().contains("no certificate"));
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
    assert!(
        result.err().unwrap().to_string().contains("no CA certificates")
    );
}

// -----------------------------------------------------------------------
// load_pem_chain
// -----------------------------------------------------------------------

#[test]
fn load_pem_chain_parses_single_cert() {
    init_crypto();
    let (cert_ders, _) = generate_self_signed(&["localhost"]).unwrap();
    let pem = cert_to_pem(&cert_ders[0]);
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(pem.as_bytes()).unwrap();
    let certs = load_pem_chain(f.path()).unwrap();
    assert_eq!(certs.len(), 1);
}

#[test]
fn load_pem_chain_parses_two_certs() {
    init_crypto();
    let (c1, _) = generate_self_signed(&["a.test"]).unwrap();
    let (c2, _) = generate_self_signed(&["b.test"]).unwrap();
    let pem = format!("{}{}", cert_to_pem(&c1[0]), cert_to_pem(&c2[0]));
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(pem.as_bytes()).unwrap();
    let certs = load_pem_chain(f.path()).unwrap();
    assert_eq!(certs.len(), 2);
}

#[test]
fn load_pem_chain_empty_file_errors() {
    let f = NamedTempFile::new().unwrap(); // empty
    let err = load_pem_chain(f.path()).unwrap_err();
    assert!(err.to_string().contains("no certificates"));
}

#[test]
fn load_pem_chain_nonexistent_file_errors() {
    let err = load_pem_chain(std::path::Path::new("/nonexistent/path.pem")).unwrap_err();
    assert!(err.to_string().contains("No such file") || err.to_string().contains("os error"));
}

// -----------------------------------------------------------------------
// parse_no_proxy
// -----------------------------------------------------------------------

#[test]
fn parse_no_proxy_none_returns_empty() {
    assert_eq!(parse_no_proxy(None), Vec::<String>::new());
}

#[test]
fn parse_no_proxy_empty_string_returns_empty() {
    assert_eq!(parse_no_proxy(Some("")), Vec::<String>::new());
}

#[test]
fn parse_no_proxy_parses_entries() {
    let result = parse_no_proxy(Some("example.com, .local, internal"));
    assert_eq!(result, vec!["example.com", ".local", "internal"]);
}

#[test]
fn parse_no_proxy_lowercases() {
    let result = parse_no_proxy(Some("EXAMPLE.COM"));
    assert_eq!(result, vec!["example.com"]);
}

// -----------------------------------------------------------------------
// no_proxy_matches
// -----------------------------------------------------------------------

#[test]
fn no_proxy_matches_empty_entries_never_matches() {
    assert!(!no_proxy_matches("example.com", &[]));
}

#[test]
fn no_proxy_matches_exact_host() {
    let entries = parse_no_proxy(Some("example.com"));
    assert!(no_proxy_matches("example.com", &entries));
}

#[test]
fn no_proxy_matches_host_with_port() {
    let entries = parse_no_proxy(Some("example.com"));
    assert!(no_proxy_matches("example.com:443", &entries));
}

#[test]
fn no_proxy_matches_subdomain_with_dot_prefix() {
    let entries = parse_no_proxy(Some(".example.com"));
    assert!(no_proxy_matches("sub.example.com", &entries));
    assert!(no_proxy_matches("deep.sub.example.com", &entries));
}

#[test]
fn no_proxy_matches_subdomain_without_dot_prefix() {
    // A no-dot entry also matches subdomains.
    let entries = parse_no_proxy(Some("example.com"));
    assert!(no_proxy_matches("sub.example.com", &entries));
}

#[test]
fn no_proxy_does_not_match_different_host() {
    let entries = parse_no_proxy(Some("example.com"));
    assert!(!no_proxy_matches("other.com", &entries));
    assert!(!no_proxy_matches("notexample.com", &entries));
}

// -----------------------------------------------------------------------
// build_http_client
// -----------------------------------------------------------------------

#[test]
fn build_http_client_default_config() {
    init_crypto();
    let client = build_http_client(HttpClientConfig {
        insecure: false,
        ca: None,
        proxy: None,
        proxy_ca: None,
        no_proxy: None,
    });
    assert!(client.is_ok());
}

#[test]
fn build_http_client_invalid_proxy_url_errors() {
    init_crypto();
    let err = build_http_client(HttpClientConfig {
        insecure: false,
        ca: None,
        proxy: Some("not a url at all !!!"),
        proxy_ca: None,
        no_proxy: None,
    })
    .unwrap_err();
    assert!(err.to_string().contains("invalid proxy URL"));
}

#[test]
fn build_http_client_with_proxy_ca_builds() {
    init_crypto();
    let (ca_pem, _, _) = gen_ca_and_client();
    let mut ca_file = NamedTempFile::new().unwrap();
    ca_file.write_all(ca_pem.as_bytes()).unwrap();
    build_http_client(HttpClientConfig {
        insecure: false,
        ca: None,
        proxy: Some("http://proxy.example.com:3128"),
        proxy_ca: Some(ca_file.path()),
        no_proxy: None,
    })
    .unwrap();
}

#[test]
fn build_http_client_proxy_env_var_fallback_builds() {
    init_crypto();
    // Safety: single-threaded test binary, no concurrent env mutation.
    unsafe { std::env::set_var("HTTPS_PROXY", "http://proxy.example.com:3128") };
    let result = build_http_client(HttpClientConfig {
        insecure: false,
        ca: None,
        proxy: None,
        proxy_ca: None,
        no_proxy: None,
    });
    unsafe { std::env::remove_var("HTTPS_PROXY") };
    result.unwrap();
}

#[test]
fn build_http_client_with_ca_builds() {
    init_crypto();
    let (ca_pem, _, _) = gen_ca_and_client();
    let mut ca_file = NamedTempFile::new().unwrap();
    ca_file.write_all(ca_pem.as_bytes()).unwrap();
    build_http_client(HttpClientConfig {
        insecure: false,
        ca: Some(ca_file.path()),
        proxy: None,
        proxy_ca: None,
        no_proxy: None,
    })
    .unwrap();
}
