// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;

use anyhow::Context;
use rustls::RootCertStore;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::WebPkiClientVerifier;
use tokio_rustls::TlsAcceptor;
use tracing::info;

use super::cert::{generate_self_signed, load_pem};

/// Build a `TlsAcceptor` from DER-encoded cert chain and key bytes.
/// No client certificate authentication.
pub fn acceptor_from_der(cert_ders: Vec<Vec<u8>>, key_der: Vec<u8>) -> anyhow::Result<TlsAcceptor> {
    let certs: Vec<CertificateDer> = cert_ders.into_iter().map(CertificateDer::from).collect();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("building rustls ServerConfig")?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// Build a `TlsAcceptor` with optional mTLS client certificate authentication.
///
/// `client_ca_ders` is the list of DER-encoded CA certificates used to verify
/// client certs.  Client certificates are **optional** — connections without a
/// client cert are accepted and fall through to other auth methods (Basic,
/// local UID).  Connections that do present a cert have it verified against the
/// supplied CA store.
pub fn acceptor_from_der_mtls(
    cert_ders: Vec<Vec<u8>>,
    key_der: Vec<u8>,
    client_ca_ders: Vec<Vec<u8>>,
) -> anyhow::Result<TlsAcceptor> {
    let certs: Vec<CertificateDer> = cert_ders.into_iter().map(CertificateDer::from).collect();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));

    let mut roots = RootCertStore::empty();
    for ca_der in client_ca_ders {
        roots
            .add(CertificateDer::from(ca_der))
            .context("adding client CA to root store")?;
    }

    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .allow_unauthenticated()
        .build()
        .context("building WebPkiClientVerifier")?;

    let config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)
        .context("building rustls ServerConfig with mTLS")?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// Load cert + key from PEM files, or auto-generate self-signed.
///
/// If both `cert_path`/`key_path` are `None` a self-signed certificate is
/// generated for `["localhost", hostname]`.  Both paths must be `Some` or both
/// `None`.
///
/// If `client_ca_path` is `Some`, mTLS is enabled: client certificates are
/// optional but, when presented, must be signed by one of the CAs in that PEM
/// file (supports chains with multiple concatenated blocks).
pub fn load_or_generate(
    cert_path: Option<&std::path::Path>,
    key_path: Option<&std::path::Path>,
    hostname: &str,
    client_ca_path: Option<&std::path::Path>,
) -> anyhow::Result<TlsAcceptor> {
    let (cert_der, key_der) = match (cert_path, key_path) {
        (Some(c), Some(k)) => load_pem(c, k)?,
        (None, None) => {
            info!("generating self-signed TLS certificate for {hostname}");
            generate_self_signed(&["localhost", hostname])?
        }
        _ => anyhow::bail!("--cert and --key must both be provided (or both omitted)"),
    };

    if let Some(ca_path) = client_ca_path {
        let pem = std::fs::read_to_string(ca_path)
            .with_context(|| format!("reading --tls-client-ca {}", ca_path.display()))?;
        let ca_ders: Vec<Vec<u8>> = rustls_pemfile::certs(&mut pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .with_context(|| format!("parsing --tls-client-ca {}", ca_path.display()))?
            .into_iter()
            .map(|c| c.to_vec())
            .collect();
        if ca_ders.is_empty() {
            anyhow::bail!("no CA certificates found in {}", ca_path.display());
        }
        info!(
            ca = %ca_path.display(),
            "mTLS client authentication enabled"
        );
        acceptor_from_der_mtls(cert_der, key_der, ca_ders)
    } else {
        acceptor_from_der(cert_der, key_der)
    }
}
