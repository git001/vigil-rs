// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use anyhow::Context;
use rcgen::{CertificateParams, KeyPair};

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

/// Load cert + key from PEM files.
/// Returns `(cert_ders, key_der)`.
pub fn load_pem(
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

/// Load all PEM certificate blocks from `path` as reqwest `Certificate` objects.
/// Supports chain files with multiple concatenated `-----BEGIN CERTIFICATE-----` blocks.
pub fn load_pem_chain(path: &std::path::Path) -> anyhow::Result<Vec<reqwest::Certificate>> {
    let pem = std::fs::read_to_string(path)?;
    let mut certs = Vec::new();
    let mut block = String::new();
    for line in pem.lines() {
        block.push_str(line);
        block.push('\n');
        if line.trim() == "-----END CERTIFICATE-----" {
            certs.push(
                reqwest::Certificate::from_pem(block.as_bytes())
                    .map_err(|e| anyhow::anyhow!("invalid PEM block: {e}"))?,
            );
            block.clear();
        }
    }
    if certs.is_empty() {
        anyhow::bail!("no certificates found in {}", path.display());
    }
    Ok(certs)
}

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
