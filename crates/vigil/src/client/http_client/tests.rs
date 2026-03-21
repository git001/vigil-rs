// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::io::Write as _;

use tempfile::NamedTempFile;

use super::{HttpConfig, build_reqwest_client};

fn cert_pem() -> String {
    use rcgen::{CertificateParams, KeyPair};
    let key = KeyPair::generate().unwrap();
    let params = CertificateParams::new(vec!["test".to_string()]).unwrap();
    params.self_signed(&key).unwrap().pem()
}

fn cert_and_key_pem() -> (String, String) {
    use rcgen::{CertificateParams, KeyPair};
    let key = KeyPair::generate().unwrap();
    let params = CertificateParams::new(vec!["test".to_string()]).unwrap();
    let cert = params.self_signed(&key).unwrap().pem();
    let key_pem = key.serialize_pem();
    (cert, key_pem)
}

fn write_tmp(content: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f
}

fn default_config() -> HttpConfig {
    HttpConfig {
        insecure: false,
        user: None,
        cert: None,
        key: None,
        cacert: None,
        proxy: None,
        proxy_cacert: None,
        no_proxy: None,
    }
}

#[test]
fn basic_config_builds() {
    build_reqwest_client(default_config()).unwrap();
}

#[test]
fn user_without_colon_returns_error() {
    let err = build_reqwest_client(HttpConfig {
        user: Some("nocolon".to_string()),
        ..default_config()
    })
    .unwrap_err();
    assert!(err.to_string().contains("username:password"));
}

#[test]
fn basic_auth_client_builds() {
    build_reqwest_client(HttpConfig {
        user: Some("alice:secret".to_string()),
        ..default_config()
    })
    .unwrap();
}

#[test]
fn cert_without_key_returns_error() {
    let (cert, _) = cert_and_key_pem();
    let cert_file = write_tmp(&cert);
    let err = build_reqwest_client(HttpConfig {
        cert: Some(cert_file.path().to_owned()),
        key: None,
        ..default_config()
    })
    .unwrap_err();
    assert!(err.to_string().contains("--cert requires --key"));
}

#[test]
fn key_without_cert_returns_error() {
    let (_, key) = cert_and_key_pem();
    let key_file = write_tmp(&key);
    let err = build_reqwest_client(HttpConfig {
        cert: None,
        key: Some(key_file.path().to_owned()),
        ..default_config()
    })
    .unwrap_err();
    assert!(err.to_string().contains("--key requires --cert"));
}

#[test]
fn mtls_client_builds() {
    let (cert, key) = cert_and_key_pem();
    let cert_file = write_tmp(&cert);
    let key_file = write_tmp(&key);
    build_reqwest_client(HttpConfig {
        cert: Some(cert_file.path().to_owned()),
        key: Some(key_file.path().to_owned()),
        ..default_config()
    })
    .unwrap();
}

#[test]
fn cacert_client_builds() {
    let pem = cert_pem();
    let ca_file = write_tmp(&pem);
    build_reqwest_client(HttpConfig {
        cacert: Some(ca_file.path().to_owned()),
        ..default_config()
    })
    .unwrap();
}

#[test]
fn proxy_cacert_client_builds() {
    let pem = cert_pem();
    let ca_file = write_tmp(&pem);
    build_reqwest_client(HttpConfig {
        proxy: Some("http://proxy.example.com:3128".to_string()),
        proxy_cacert: Some(ca_file.path().to_owned()),
        ..default_config()
    })
    .unwrap();
}

#[test]
fn invalid_proxy_url_returns_error() {
    let err = build_reqwest_client(HttpConfig {
        proxy: Some("not a url !!!".to_string()),
        ..default_config()
    })
    .unwrap_err();
    assert!(err.to_string().contains("invalid proxy URL"));
}

#[test]
fn proxy_env_var_fallback_builds() {
    // HTTPS_PROXY env var should be picked up when proxy is None.
    // Safety: single-threaded test binary, no concurrent env mutation.
    unsafe { std::env::set_var("HTTPS_PROXY", "http://proxy.example.com:3128") };
    let result = build_reqwest_client(default_config());
    unsafe { std::env::remove_var("HTTPS_PROXY") };
    result.unwrap();
}

#[test]
fn no_proxy_with_proxy_builds() {
    build_reqwest_client(HttpConfig {
        proxy: Some("http://proxy.example.com:3128".to_string()),
        no_proxy: Some("localhost,127.0.0.1,10.0.0.0/8".to_string()),
        ..default_config()
    })
    .unwrap();
}
