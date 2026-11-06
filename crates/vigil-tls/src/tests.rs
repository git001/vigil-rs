// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::path::{Path, PathBuf};

use super::*;

/// A self-signed CA certificate (PEM) and the key it was made with.
fn self_signed() -> (String, rcgen::KeyPair) {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["vigil-test".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    (cert.pem(), key)
}

fn write(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, content).unwrap();
    p
}

#[test]
fn ca_bundle_reads_one_or_more_certificates() {
    let dir = tempfile::tempdir().unwrap();
    let (a, _) = self_signed();
    let (b, _) = self_signed();
    let one = write(dir.path(), "one.pem", a.as_bytes());
    let two = write(dir.path(), "two.pem", format!("{a}{b}").as_bytes());
    assert_eq!(ca_bundle(PemFile::new("tls.ca", &one)).unwrap().len(), 1);
    assert_eq!(ca_bundle(PemFile::new("tls.ca", &two)).unwrap().len(), 2);
}

#[test]
fn errors_name_the_setting_and_path_never_the_content() {
    let dir = tempfile::tempdir().unwrap();
    let junk = write(dir.path(), "junk.pem", b"MY-pw1! is not a certificate\n");
    let err = format!(
        "{:#}",
        ca_bundle(PemFile::new("--cacert", &junk)).unwrap_err()
    );
    assert!(err.starts_with("--cacert \""), "{err}");
    assert!(err.contains("no PEM certificate found"), "{err}");
    assert!(!err.contains("MY-pw1!"), "{err}");

    let empty = write(dir.path(), "empty.pem", b"");
    let err = format!(
        "{:#}",
        ca_bundle(PemFile::new("--cacert", &empty)).unwrap_err()
    );
    assert!(err.contains("no PEM certificate found"), "{err}");

    let missing = dir.path().join("missing.pem");
    let err = format!(
        "{:#}",
        ca_bundle(PemFile::new("tls.ca", &missing)).unwrap_err()
    );
    assert!(
        err.contains("tls.ca") && err.contains("missing.pem"),
        "{err}"
    );

    let err = format!(
        "{:#}",
        ca_bundle(PemFile::new("tls.ca", dir.path())).unwrap_err()
    );
    assert!(err.contains("not a regular file"), "{err}");
}

#[test]
fn oversized_files_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let big = write(
        dir.path(),
        "big.pem",
        &vec![b'A'; MAX_PEM_FILE as usize + 1],
    );
    let err = format!("{:#}", read_pem(PemFile::new("tls.ca", &big)).unwrap_err());
    assert!(err.contains("larger than 1024 KiB"), "{err}");
}

#[test]
fn layer_settings_need_absolute_paths() {
    let err = format!(
        "{:#}",
        ca_bundle(PemFile::layer("tls.ca", Path::new("certs/ca.pem"))).unwrap_err()
    );
    assert_eq!(err, "tls.ca \"certs/ca.pem\": path must be absolute");
}

#[test]
fn cert_and_key_keep_the_key_type() {
    let dir = tempfile::tempdir().unwrap();
    let (cert, key) = self_signed();
    // Without a trailing newline: the joined PEM must still be valid.
    let cert_path = write(dir.path(), "tls.crt", cert.trim_end().as_bytes());
    let key_path = write(dir.path(), "tls.key", key.serialize_pem().as_bytes());
    let ck = cert_and_key(
        PemFile::new("--cert", &cert_path),
        PemFile::new("--key", &key_path),
    )
    .unwrap();
    assert_eq!(ck.chain.len(), 1);
    assert!(matches!(ck.key, PrivateKeyDer::Pkcs8(_)));
    ck.reqwest_identity().unwrap();

    let err = format!(
        "{:#}",
        cert_and_key(
            PemFile::new("--cert", &cert_path),
            PemFile::new("--key", &cert_path)
        )
        .unwrap_err()
    );
    assert!(
        err.contains("--key") && err.contains("no private key found"),
        "{err}"
    );
}

#[test]
fn client_tls_needs_cert_and_key_together() {
    let dir = tempfile::tempdir().unwrap();
    let (cert, _) = self_signed();
    let cert_path = write(dir.path(), "tls.crt", cert.as_bytes());
    let tls = ClientTls {
        cert: Some(PemFile::new("tls.cert", &cert_path)),
        ..Default::default()
    };
    let err = format!("{:#}", tls.apply(reqwest::Client::builder()).unwrap_err());
    assert_eq!(err, "tls.cert needs a key as well");
}

#[test]
fn client_tls_applies_ca_and_identity() {
    let dir = tempfile::tempdir().unwrap();
    let (cert, key) = self_signed();
    let ca = write(dir.path(), "ca.pem", cert.as_bytes());
    let cert_path = write(dir.path(), "tls.crt", cert.as_bytes());
    let key_path = write(dir.path(), "tls.key", key.serialize_pem().as_bytes());
    let tls = ClientTls {
        insecure: false,
        ca: vec![PemFile::new("tls.ca", &ca)],
        cert: Some(PemFile::new("tls.cert", &cert_path)),
        key: Some(PemFile::new("tls.key", &key_path)),
    };
    tls.apply(reqwest::Client::builder())
        .unwrap()
        .build()
        .unwrap();
}
