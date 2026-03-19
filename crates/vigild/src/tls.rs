// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;

use anyhow::Context;
use rcgen::{CertificateParams, KeyPair};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::TlsAcceptor;
use tracing::info;

// ---------------------------------------------------------------------------
// Certificate generation
// ---------------------------------------------------------------------------

/// Generate a self-signed ECDSA P-256 certificate.
/// Returns `(cert_chain, key_der)`.
pub fn generate_self_signed(hostnames: &[&str]) -> anyhow::Result<(Vec<Vec<u8>>, Vec<u8>)> {
    let names: Vec<String> = hostnames.iter().map(|s| s.to_string()).collect();
    let key_pair = KeyPair::generate().context("generating key pair")?;
    let cert = CertificateParams::new(names)
        .context("building cert params")?
        .self_signed(&key_pair)
        .context("self-signing certificate")?;
    Ok((vec![cert.der().to_vec()], key_pair.serialize_der()))
}

// ---------------------------------------------------------------------------
// TlsAcceptor construction
// ---------------------------------------------------------------------------

/// Build a `TlsAcceptor` from DER-encoded cert chain and key bytes.
pub fn acceptor_from_der(cert_ders: Vec<Vec<u8>>, key_der: Vec<u8>) -> anyhow::Result<TlsAcceptor> {
    let certs: Vec<CertificateDer> = cert_ders.into_iter().map(CertificateDer::from).collect();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("building rustls ServerConfig")?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// Load cert + key from PEM files, or auto-generate self-signed.
///
/// If both paths are `None` a self-signed certificate is generated for
/// `["localhost", hostname]`.  Both paths must be `Some` or both `None`.
pub fn load_or_generate(
    cert_path: Option<&std::path::Path>,
    key_path: Option<&std::path::Path>,
    hostname: &str,
) -> anyhow::Result<TlsAcceptor> {
    let (cert_der, key_der) = match (cert_path, key_path) {
        (Some(c), Some(k)) => load_pem(c, k)?,
        (None, None) => {
            info!("generating self-signed TLS certificate for {hostname}");
            generate_self_signed(&["localhost", hostname])?
        }
        _ => anyhow::bail!("--cert and --key must both be provided (or both omitted)"),
    };
    acceptor_from_der(cert_der, key_der)
}

fn load_pem(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> anyhow::Result<(Vec<Vec<u8>>, Vec<u8>)> {
    // ---- certificate (chain) ----
    let cert_pem =
        std::fs::read_to_string(cert_path).with_context(|| format!("reading {cert_path:?}"))?;
    let cert_ders: Vec<Vec<u8>> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parsing certificates in {cert_path:?}"))?
        .into_iter()
        .map(|c| c.to_vec())
        .collect();
    if cert_ders.is_empty() {
        anyhow::bail!("no certificate found in {cert_path:?}");
    }

    // ---- private key ----
    let key_pem =
        std::fs::read_to_string(key_path).with_context(|| format!("reading {key_path:?}"))?;
    let key_der = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .with_context(|| format!("parsing private key {key_path:?}"))?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {key_path:?}"))?
        .secret_der()
        .to_vec();

    Ok((cert_ders, key_der))
}

// ---------------------------------------------------------------------------
// PEM export (for serving the CA cert to clients)
// ---------------------------------------------------------------------------

/// Re-encode a DER cert as PEM so clients can pin it.
#[allow(dead_code)]
pub fn cert_to_pem(cert_der: &[u8]) -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let b64 = STANDARD.encode(cert_der);
    let lines: String = b64
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    format!("-----BEGIN CERTIFICATE-----\n{lines}\n-----END CERTIFICATE-----\n")
}
