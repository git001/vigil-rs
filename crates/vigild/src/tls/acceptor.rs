// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;

use anyhow::Context;
use rustls::RootCertStore;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use tokio_rustls::TlsAcceptor;
use tracing::info;

use vigil_tls::PemFile;

use super::cert::generate_self_signed;

/// Build a `TlsAcceptor` from a certificate chain and its key.
/// No client certificate authentication.
pub fn acceptor_from_der(
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> anyhow::Result<TlsAcceptor> {
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("building rustls ServerConfig")?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// Build a `TlsAcceptor` with optional mTLS client certificate authentication.
///
/// `client_cas` are the CA certificates used to verify
/// client certs.  Client certificates are **optional** — connections without a
/// client cert are accepted and fall through to other auth methods (Basic,
/// local UID).  Connections that do present a cert have it verified against the
/// supplied CA store.
pub fn acceptor_from_der_mtls(
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    client_cas: Vec<CertificateDer<'static>>,
) -> anyhow::Result<TlsAcceptor> {
    let mut roots = RootCertStore::empty();
    for ca in client_cas {
        roots.add(ca).context("adding client CA to root store")?;
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
    let (certs, key) = match (cert_path, key_path) {
        (Some(c), Some(k)) => {
            let ck = vigil_tls::cert_and_key(PemFile::new("--cert", c), PemFile::new("--key", k))?;
            (ck.chain, ck.key)
        }
        (None, None) => {
            info!("generating self-signed TLS certificate for {hostname}");
            generate_self_signed(&["localhost", hostname])?
        }
        _ => anyhow::bail!("--cert and --key must both be provided (or both omitted)"),
    };

    if let Some(ca_path) = client_ca_path {
        let cas = vigil_tls::ca_bundle(PemFile::new("--tls-client-ca", ca_path))?;
        info!(
            ca = %ca_path.display(),
            "mTLS client authentication enabled"
        );
        acceptor_from_der_mtls(certs, key, cas)
    } else {
        acceptor_from_der(certs, key)
    }
}
