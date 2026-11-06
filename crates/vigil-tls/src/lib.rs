// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! TLS material for vigild, vigil and vigil-log-relay, in one place: reading
//! CA bundles, a certificate with its key, and applying them to an HTTPS
//! client.
//!
//! Files are read with vigil's file handling ([`vigil_files`]): a regular
//! file (symlinks are followed, as in Kubernetes Secret volumes), at most
//! [`MAX_PEM_FILE`], absolute paths for settings from layer files. Errors
//! name the setting (e.g. `--cacert`, `tls.ca`) and the path, never the
//! content.

use std::path::Path;

use anyhow::Context;

pub use rustls_pki_types::{CertificateDer, PrivateKeyDer};

/// Largest PEM file read (a CA bundle of a few hundred certificates fits).
pub const MAX_PEM_FILE: u64 = 1024 * 1024;

/// A file setting: its name for errors (`--cacert`, `tls.ca`), the path and
/// which paths it accepts.
#[derive(Debug, Clone, Copy)]
pub struct PemFile<'a> {
    pub name: &'a str,
    pub path: &'a Path,
    pub rule: vigil_files::PathRule,
}

impl<'a> PemFile<'a> {
    /// A command-line option: relative paths are allowed.
    pub fn new(name: &'a str, path: &'a Path) -> Self {
        Self {
            name,
            path,
            rule: vigil_files::PathRule::Any,
        }
    }

    /// A setting in a layer file: the path must be absolute.
    pub fn layer(name: &'a str, path: &'a Path) -> Self {
        Self {
            name,
            path,
            rule: vigil_files::PathRule::Absolute,
        }
    }
}

/// Read a PEM file (see the module docs).
pub fn read_pem(file: PemFile<'_>) -> anyhow::Result<Vec<u8>> {
    vigil_files::read_existing(file.path, MAX_PEM_FILE, file.rule)
        .map_err(|e| anyhow::anyhow!("{} {e}", file.name))
}

/// The certificates in `pem`; at least one. `name` labels errors.
pub fn certs_from_pem(name: &str, pem: &[u8]) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let certs = rustls_pemfile::certs(&mut &pem[..])
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("{name}: invalid PEM"))?;
    anyhow::ensure!(!certs.is_empty(), "{name}: no PEM certificate found");
    Ok(certs)
}

/// A CA certificate or chain (several concatenated PEM blocks) from a file.
pub fn ca_bundle(file: PemFile<'_>) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let pem = read_pem(file)?;
    certs_from_pem(&format!("{} {:?}", file.name, file.path), &pem)
}

/// A certificate (chain) and its private key, e.g. a server certificate or
/// a TLS client certificate (mTLS).
#[derive(Debug)]
pub struct CertKey {
    pub chain: Vec<CertificateDer<'static>>,
    /// Keeps its type (PKCS#8, PKCS#1 RSA or SEC1 EC).
    pub key: PrivateKeyDer<'static>,
    /// Certificate and key PEM, one after the other (for reqwest).
    pem: Vec<u8>,
}

impl CertKey {
    /// The pair as a reqwest client identity (mTLS).
    pub fn reqwest_identity(&self) -> anyhow::Result<reqwest::Identity> {
        reqwest::Identity::from_pem(&self.pem).context("TLS client certificate and key")
    }
}

/// Read a certificate (chain) and its key.
pub fn cert_and_key(cert: PemFile<'_>, key: PemFile<'_>) -> anyhow::Result<CertKey> {
    let cert_pem = read_pem(cert)?;
    let chain = certs_from_pem(&format!("{} {:?}", cert.name, cert.path), &cert_pem)?;
    let key_pem = read_pem(key)?;
    let key_der = rustls_pemfile::private_key(&mut &key_pem[..])
        .with_context(|| format!("{} {:?}: invalid PEM", key.name, key.path))?
        .with_context(|| format!("{} {:?}: no private key found", key.name, key.path))?;
    let mut pem = cert_pem;
    if !pem.ends_with(b"\n") {
        pem.push(b'\n');
    }
    pem.extend_from_slice(&key_pem);
    Ok(CertKey {
        chain,
        key: key_der,
        pem,
    })
}

/// TLS settings of an outgoing HTTPS connection.
#[derive(Debug, Default, Clone)]
pub struct ClientTls<'a> {
    /// Do not verify the server certificate (self-signed, tests).
    pub insecure: bool,
    /// Extra CAs to trust, besides the built-in roots.
    pub ca: Vec<PemFile<'a>>,
    /// Client certificate and key (mTLS); both or neither.
    pub cert: Option<PemFile<'a>>,
    pub key: Option<PemFile<'a>>,
}

impl ClientTls<'_> {
    /// Apply the settings to a reqwest client builder (reads the files).
    pub fn apply(&self, mut b: reqwest::ClientBuilder) -> anyhow::Result<reqwest::ClientBuilder> {
        b = b.danger_accept_invalid_certs(self.insecure);
        for file in &self.ca {
            for cert in ca_bundle(*file)? {
                let cert = reqwest::Certificate::from_der(&cert)
                    .with_context(|| format!("{} {:?}", file.name, file.path))?;
                b = b.add_root_certificate(cert);
            }
        }
        match (self.cert, self.key) {
            (Some(c), Some(k)) => b = b.identity(cert_and_key(c, k)?.reqwest_identity()?),
            (None, None) => {}
            (Some(c), None) => anyhow::bail!("{} needs a key as well", c.name),
            (None, Some(k)) => anyhow::bail!("{} needs a certificate as well", k.name),
        }
        Ok(b)
    }
}

#[cfg(test)]
mod tests;
