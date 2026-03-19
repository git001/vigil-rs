//! Kubernetes client builder — applies proxy / TLS settings from CLI.

use std::path::Path;

use anyhow::{Context, Result};
use kube::Client;

use crate::cli::Cli;

// ---------------------------------------------------------------------------
// Client construction
// ---------------------------------------------------------------------------

pub async fn build(cli: &Cli) -> Result<Client> {
    let mut config = kube::Config::infer()
        .await
        .context("failed to infer Kubernetes config — is this running in-cluster?")?;

    if let Some(proxy_url) = &cli.source_proxy {
        config.proxy_url = Some(
            proxy_url.parse().context("invalid --source-proxy URL")?,
        );
    }
    if cli.source_proxy_insecure {
        config.accept_invalid_certs = true;
    }
    if let Some(ca_path) = &cli.source_proxy_cacert {
        let der_certs = load_der_certs(ca_path)
            .with_context(|| format!("failed to load --source-proxy-cacert {}", ca_path.display()))?;
        config.root_cert.get_or_insert_with(Vec::new).extend(der_certs);
    }

    Client::try_from(config).context("failed to create Kubernetes client")
}

// ---------------------------------------------------------------------------
// PEM → DER certificate chain loader for kube
// ---------------------------------------------------------------------------

/// Parse all PEM certificate blocks from `path` and return them as DER bytes.
/// Supports chain files with multiple concatenated `-----BEGIN CERTIFICATE-----` blocks.
pub fn load_der_certs(path: &Path) -> Result<Vec<Vec<u8>>> {
    let pem = std::fs::read(path)?;
    let certs: Vec<_> = rustls_pemfile::certs(&mut pem.as_slice())
        .collect::<std::result::Result<_, _>>()
        .context("invalid PEM certificate")?;
    if certs.is_empty() {
        anyhow::bail!("no certificates found in {}", path.display());
    }
    Ok(certs.into_iter().map(|c| c.to_vec()).collect())
}
