// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `identities:` from the layer files: checked and, for
//! `basic.password-hash-file` and `tls.ca`, read while the layers are
//! loaded (off the async runtime).

use std::path::{Path, PathBuf};

use anyhow::Context;
use indexmap::IndexMap;
use vigil_types::identity::{BasicIdentity, Identity, IdentitySource, TlsIdentity};
use vigil_types::plan::{BasicConfig, IdentityConfig, IdentityTls};

/// Largest `password-hash-file` read.
const MAX_HASH_FILE: u64 = 64 * 1024;

/// The layer identities, ready for [`super::IdentityStore::set_layer`].
#[derive(Debug, Default)]
pub struct LoadedIdentities {
    pub identities: Vec<Identity>,
    /// Files they were read from; `--watch-layers` watches them too, so a
    /// rotated Kubernetes Secret is picked up.
    pub files: Vec<PathBuf>,
}

/// Check every identity and read the files it names. Errors name the
/// identity, its layer and the problem, never a file's content.
pub fn load_layer_identities(
    configs: &IndexMap<String, IdentityConfig>,
) -> anyhow::Result<LoadedIdentities> {
    let mut out = LoadedIdentities::default();
    for (name, cfg) in configs {
        let id = load_one(name, cfg, &mut out.files)
            .with_context(|| format!("identity {name:?} (layer {:?})", cfg.layer))?;
        out.identities.push(id);
    }
    Ok(out)
}

fn load_one(
    name: &str,
    cfg: &IdentityConfig,
    files: &mut Vec<PathBuf>,
) -> anyhow::Result<Identity> {
    anyhow::ensure!(
        cfg.local.is_some() || cfg.basic.is_some() || cfg.tls.is_some(),
        "needs at least one of local, basic or tls"
    );
    Ok(Identity {
        name: name.to_owned(),
        access: cfg.access,
        local: cfg.local.clone(),
        basic: cfg.basic.as_ref().map(|b| basic(b, files)).transpose()?,
        tls: cfg.tls.as_ref().map(|t| tls(t, files)).transpose()?,
        source: IdentitySource::Layer,
        layer: Some(cfg.layer.clone()),
    })
}

fn basic(b: &BasicConfig, files: &mut Vec<PathBuf>) -> anyhow::Result<BasicIdentity> {
    let hash = match (&b.password_hash, &b.password_hash_file) {
        (Some(v), None) => v.clone(),
        (None, Some(path)) => {
            let hash = read_hash_file(path)?;
            files.push(path.clone());
            hash
        }
        (Some(_), Some(_)) => {
            anyhow::bail!("basic: set either password-hash or password-hash-file, not both")
        }
        (None, None) => anyhow::bail!("basic: password-hash or password-hash-file is required"),
    };
    // Kubernetes Secrets often end with a newline.
    let hash = hash.trim().to_owned();
    anyhow::ensure!(
        hash.starts_with("$6$"),
        "basic: password-hash is not a SHA-512 crypt hash (\"$6$…\", \
         e.g. from `openssl passwd -6`)"
    );
    Ok(BasicIdentity {
        password_hash: hash,
    })
}

/// The hash file with the rules of `file()`: an absolute path, a regular
/// file, UTF-8; missing or empty is an error.
fn read_hash_file(path: &Path) -> anyhow::Result<String> {
    let content = vigil_files::read_text(path, MAX_HASH_FILE, vigil_files::PathRule::Absolute)
        .map_err(|e| anyhow::anyhow!("basic.password-hash-file {e}"))?
        .with_context(|| format!("basic.password-hash-file {path:?}: No such file or directory"))?;
    anyhow::ensure!(
        !content.trim().is_empty(),
        "basic.password-hash-file {path:?}: is empty"
    );
    Ok(content)
}

/// All CA files as one PEM bundle; each must hold a certificate.
fn tls(t: &IdentityTls, files: &mut Vec<PathBuf>) -> anyhow::Result<TlsIdentity> {
    anyhow::ensure!(!t.ca.is_empty(), "tls.ca: at least one file is required");
    let mut bundle = String::new();
    for path in &t.ca {
        let file = vigil_tls::PemFile::layer("tls.ca", path);
        let pem = vigil_tls::read_pem(file)?;
        vigil_tls::certs_from_pem(&format!("tls.ca {path:?}"), &pem)?;
        bundle.push_str(
            std::str::from_utf8(&pem).with_context(|| format!("tls.ca {path:?}: not UTF-8"))?,
        );
        if !bundle.ends_with('\n') {
            bundle.push('\n');
        }
        files.push(path.clone());
    }
    Ok(TlsIdentity { ca_cert: bundle })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vigil_types::identity::{IdentityAccess, LocalIdentity};

    /// `openssl passwd -6 -salt testextra extrapass`
    const HASH: &str = "$6$testextra$Cq1GhPxDNtKxM7J.P1EovJ64dyEsjMy59Cr2zHgWhpmUsETC.YtW8GFsrGO810Kh5f/9.SYG53plbhT3VaA7z/";

    fn cfg(access: IdentityAccess) -> IdentityConfig {
        IdentityConfig {
            layer: "001-auth".into(),
            access,
            local: None,
            basic: None,
            tls: None,
        }
    }

    fn load(name: &str, c: IdentityConfig) -> anyhow::Result<LoadedIdentities> {
        load_layer_identities(&[(name.to_string(), c)].into_iter().collect())
    }

    fn ca_pem(cn: &str) -> String {
        let key = rcgen::KeyPair::generate().unwrap();
        rcgen::CertificateParams::new(vec![cn.into()])
            .unwrap()
            .self_signed(&key)
            .unwrap()
            .pem()
    }

    fn basic(hash: Option<&str>, file: Option<&Path>) -> IdentityConfig {
        let mut c = cfg(IdentityAccess::Read);
        c.basic = Some(BasicConfig {
            password_hash: hash.map(Into::into),
            password_hash_file: file.map(Into::into),
        });
        c
    }

    #[test]
    fn local_and_inline_basic_load_as_layer_identities() {
        let mut admin = cfg(IdentityAccess::Admin);
        admin.local = Some(LocalIdentity::default());
        let out = load("ops-admin", admin).unwrap();
        let id = &out.identities[0];
        assert_eq!(id.source, IdentitySource::Layer);
        assert_eq!(id.layer.as_deref(), Some("001-auth"));
        assert_eq!(id.local.as_ref().unwrap().user_id, None);

        let out = load("log-relay", basic(Some(HASH), None)).unwrap();
        assert_eq!(
            out.identities[0].basic.as_ref().unwrap().password_hash,
            HASH
        );
        assert!(out.files.is_empty());
    }

    #[test]
    fn password_hash_file_is_read_and_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("log-relay.hash");
        std::fs::write(&file, format!("{HASH}\n")).unwrap();
        let out = load("log-relay", basic(None, Some(&file))).unwrap();
        assert_eq!(
            out.identities[0].basic.as_ref().unwrap().password_hash,
            HASH
        );
        assert_eq!(out.files, [file]);
    }

    #[test]
    fn tls_ca_files_form_one_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("monitoring-ca.pem");
        let b = dir.path().join("backup-ca.pem");
        std::fs::write(&a, ca_pem("monitoring-ca")).unwrap();
        // Without a trailing newline: the bundle still separates the files.
        std::fs::write(&b, ca_pem("backup-ca").trim_end()).unwrap();
        let mut mon = cfg(IdentityAccess::Metrics);
        mon.tls = Some(IdentityTls {
            ca: vec![a.clone(), b.clone()],
        });
        let out = load("monitoring", mon).unwrap();
        let bundle = &out.identities[0].tls.as_ref().unwrap().ca_cert;
        let certs = vigil_tls::certs_from_pem("bundle", bundle.as_bytes()).unwrap();
        assert_eq!(certs.len(), 2);
        assert_eq!(out.files, [a, b]);
    }

    #[test]
    fn errors_name_identity_layer_and_problem() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.hash");
        std::fs::write(&empty, "\n").unwrap();
        let not_pem = dir.path().join("not-a-ca.pem");
        std::fs::write(&not_pem, "MY-pw1!\n").unwrap();
        let tls = |ca: Vec<PathBuf>| {
            let mut c = cfg(IdentityAccess::Metrics);
            c.tls = Some(IdentityTls { ca });
            c
        };
        let missing = Path::new("/nonexistent/vigil/log-relay.hash");
        for (c, want) in [
            (
                cfg(IdentityAccess::Read),
                "needs at least one of local, basic or tls".to_string(),
            ),
            (
                basic(None, None),
                "password-hash or password-hash-file is required".into(),
            ),
            (
                basic(Some(HASH), Some(missing)),
                "set either password-hash or password-hash-file, not both".into(),
            ),
            (
                basic(Some("MY-pw1!"), None),
                "is not a SHA-512 crypt hash".into(),
            ),
            (
                basic(None, Some(missing)),
                format!("basic.password-hash-file {missing:?}: No such file or directory"),
            ),
            (basic(None, Some(&empty)), "is empty".into()),
            (
                basic(None, Some(Path::new("relative/log-relay.hash"))),
                "basic.password-hash-file".into(),
            ),
            (tls(vec![]), "tls.ca: at least one file is required".into()),
            (tls(vec![not_pem.clone()]), format!("tls.ca {not_pem:?}")),
            (tls(vec!["etc/ca.pem".into()]), "tls.ca".into()),
        ] {
            let err = format!("{:#}", load("log-relay", c).unwrap_err());
            assert!(
                err.starts_with("identity \"log-relay\" (layer \"001-auth\"): "),
                "{err}"
            );
            assert!(err.contains(&want), "{want}: {err}");
            assert!(!err.contains("MY-pw1!"), "never the value: {err}");
        }
    }
}
