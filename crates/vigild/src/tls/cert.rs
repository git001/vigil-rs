// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use anyhow::Context;
use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// Generate a self-signed ECDSA P-256 certificate and its key.
pub fn generate_self_signed(
    hostnames: &[&str],
) -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let names: Vec<String> = hostnames.iter().map(|s| s.to_string()).collect();
    let key_pair = KeyPair::generate().context("generating key pair")?;
    let cert = CertificateParams::new(names)
        .context("building cert params")?
        .self_signed(&key_pair)
        .context("self-signing certificate")?;
    Ok((
        vec![cert.der().clone()],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der())),
    ))
}

/// Re-encode a DER cert as PEM (tests write certificate files with it).
#[cfg(test)]
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
