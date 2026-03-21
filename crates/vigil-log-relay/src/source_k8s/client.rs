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
        config.proxy_url = Some(proxy_url.parse().context("invalid --source-proxy URL")?);
    }
    if cli.source_proxy_insecure {
        config.accept_invalid_certs = true;
    }
    if let Some(ca_path) = &cli.source_proxy_cacert {
        let der_certs = load_der_certs(ca_path).with_context(|| {
            format!("failed to load --source-proxy-cacert {}", ca_path.display())
        })?;
        config
            .root_cert
            .get_or_insert_with(Vec::new)
            .extend(der_certs);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tempfile::NamedTempFile;

    fn self_signed_pem() -> String {
        use rcgen::{CertificateParams, KeyPair};
        let key = KeyPair::generate().unwrap();
        let params = CertificateParams::new(vec!["test".to_string()]).unwrap();
        params.self_signed(&key).unwrap().pem()
    }

    #[test]
    fn load_der_certs_single_cert() {
        let pem = self_signed_pem();
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(pem.as_bytes()).unwrap();
        let certs = load_der_certs(f.path()).unwrap();
        assert_eq!(certs.len(), 1);
        assert!(!certs[0].is_empty());
    }

    #[test]
    fn load_der_certs_chain_two_certs() {
        let pem1 = self_signed_pem();
        let pem2 = self_signed_pem();
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(format!("{pem1}{pem2}").as_bytes()).unwrap();
        let certs = load_der_certs(f.path()).unwrap();
        assert_eq!(certs.len(), 2);
    }

    #[test]
    fn load_der_certs_empty_file_errors() {
        let f = NamedTempFile::new().unwrap();
        let err = load_der_certs(f.path()).unwrap_err();
        assert!(err.to_string().contains("no certificates"));
    }

    #[test]
    fn load_der_certs_nonexistent_file_errors() {
        let err = load_der_certs(std::path::Path::new("/nonexistent/path.pem")).unwrap_err();
        assert!(err.to_string().contains("os error") || err.to_string().contains("No such file"));
    }

    #[test]
    fn load_der_certs_no_pem_blocks_errors() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"this is plain text, not a PEM file\n").unwrap();
        let err = load_der_certs(f.path()).unwrap_err();
        assert!(err.to_string().contains("no certificates"));
    }
}
