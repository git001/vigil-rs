//! Kubernetes client builder — applies proxy / TLS settings from CLI.

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
    apply_proxy(&mut config, cli).await?;
    Client::try_from(config).context("failed to create Kubernetes client")
}

/// Apply `--source-proxy`, `--source-proxy-insecure` and
/// `--source-proxy-cacert` to `config`. The proxy CA is added to the CAs of
/// the kubeconfig (kube has no TLS settings of its own for the proxy).
async fn apply_proxy(config: &mut kube::Config, cli: &Cli) -> Result<()> {
    if let Some(proxy_url) = &cli.conn.source_proxy {
        config.proxy_url = Some(proxy_url.parse().context("invalid --source-proxy URL")?);
    }
    if cli.conn.source_proxy_insecure {
        config.accept_invalid_certs = true;
    }
    if let Some(ca_path) = cli.conn.source_proxy_cacert.clone() {
        let certs = vigil_files::off_runtime("TLS files", move || {
            vigil_tls::ca_bundle(vigil_tls::PemFile::new("--source-proxy-cacert", &ca_path))
        })
        .await?;
        config
            .root_cert
            .get_or_insert_with(Vec::new)
            .extend(certs.into_iter().map(|c| c.to_vec()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;

    fn cli(args: &[&str]) -> Cli {
        let mut argv = vec!["vigil-log-relay", "--stdout-sink", "--kubernetes"];
        argv.extend_from_slice(args);
        let cli = Cli::try_parse_from(argv).unwrap();
        cli.validate().unwrap();
        cli
    }

    fn config_with_ca(der: &[u8]) -> kube::Config {
        let mut c = kube::Config::new("https://10.0.0.1:6443".parse().unwrap());
        c.root_cert = Some(vec![der.to_vec()]);
        c
    }

    fn self_signed_pem() -> String {
        let key = rcgen::KeyPair::generate().unwrap();
        rcgen::CertificateParams::new(vec!["proxy.example.com".into()])
            .unwrap()
            .self_signed(&key)
            .unwrap()
            .pem()
    }

    #[tokio::test]
    async fn proxy_ca_is_added_to_the_kubeconfig_cas() {
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("proxy-ca.pem");
        std::fs::write(&ca, format!("{}{}", self_signed_pem(), self_signed_pem())).unwrap();
        let mut config = config_with_ca(b"cluster-ca");
        apply_proxy(
            &mut config,
            &cli(&[
                "--source-proxy",
                "http://proxy.example.com:3128",
                "--source-proxy-cacert",
                ca.to_str().unwrap(),
            ]),
        )
        .await
        .unwrap();
        let roots = config.root_cert.unwrap();
        assert_eq!(roots.len(), 3, "cluster CA kept, both proxy CAs added");
        assert_eq!(roots[0], b"cluster-ca");
        assert_eq!(
            config.proxy_url.unwrap().to_string(),
            "http://proxy.example.com:3128/"
        );
        assert!(!config.accept_invalid_certs);
    }

    #[tokio::test]
    async fn proxy_insecure_and_errors() {
        let mut config = config_with_ca(b"cluster-ca");
        apply_proxy(&mut config, &cli(&["--source-proxy-insecure"]))
            .await
            .unwrap();
        assert!(config.accept_invalid_certs);
        assert_eq!(config.root_cert.unwrap().len(), 1, "unchanged");

        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, "").unwrap();
        let err = apply_proxy(
            &mut config_with_ca(b"cluster-ca"),
            &cli(&["--source-proxy-cacert", empty.to_str().unwrap()]),
        )
        .await
        .unwrap_err();
        let err = format!("{err:#}");
        assert!(
            err.starts_with("--source-proxy-cacert ") && err.contains("no PEM certificate found"),
            "{err}"
        );
    }
}
