// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::collections::HashMap;

use clap::Subcommand;
use vigil_client::client::VigilClient;
use vigil_types::identity::{
    Identity, IdentityAccess, IdentitySource, IdentitySpec, LocalIdentity,
};

#[derive(Debug, Subcommand)]
pub enum IdentitiesCmd {
    /// List identities.
    #[command(name = "list")]
    List {
        /// Filter by name(s).
        names: Vec<String>,
    },

    /// Add or update a local (Unix-socket UID) identity.
    #[command(name = "add-local")]
    AddLocal {
        /// Identity name.
        name: String,
        /// Access level: metrics, read, write, or admin.
        #[arg(long, default_value = "read")]
        access: String,
        /// Restrict to a specific UID (omit to allow any local user).
        #[arg(long)]
        uid: Option<u32>,
    },

    /// Remove identities by name.
    #[command(name = "remove")]
    Remove {
        /// Names of the identities to remove.
        #[arg(required = true)]
        names: Vec<String>,
    },
}

pub async fn run(client: &VigilClient, sub: IdentitiesCmd) -> anyhow::Result<()> {
    match sub {
        IdentitiesCmd::List { names } => {
            let mut ids = client.list_identities(&names).await?;
            if ids.is_empty() {
                println!("No identities.");
                return Ok(());
            }
            ids.sort_by(|a, b| a.name.cmp(&b.name));
            println!("{:<24} {:<8} {:<24} Source", "Name", "Access", "Auth");
            println!("{}", "-".repeat(72));
            for id in &ids {
                println!(
                    "{:<24} {:<8} {:<24} {}",
                    id.name,
                    format!("{:?}", id.access).to_lowercase(),
                    auth_methods(id),
                    source(id),
                );
            }
        }

        IdentitiesCmd::AddLocal { name, access, uid } => {
            let access_level = parse_access(&access)?;
            let mut identities = HashMap::new();
            identities.insert(
                name.clone(),
                IdentitySpec {
                    access: access_level,
                    local: Some(LocalIdentity { user_id: uid }),
                    basic: None,
                    tls: None,
                },
            );
            client.add_identities(identities).await?;
            println!("Identity '{}' added.", name);
        }

        IdentitiesCmd::Remove { names } => {
            let removed = client.remove_identities(names).await?;
            if removed.is_empty() {
                println!("No identities removed.");
            } else {
                println!("Removed: {}", removed.join(", "));
            }
        }
    }
    Ok(())
}

fn parse_access(s: &str) -> anyhow::Result<IdentityAccess> {
    match s {
        "metrics" => Ok(IdentityAccess::Metrics),
        "read" => Ok(IdentityAccess::Read),
        "write" => Ok(IdentityAccess::Write),
        "admin" => Ok(IdentityAccess::Admin),
        other => anyhow::bail!(
            "unknown access level '{}': use metrics, read, write, or admin",
            other
        ),
    }
}

/// `local(any)`, `local(uid=1000)`, `basic`, `tls`, comma-separated.
fn auth_methods(id: &Identity) -> String {
    let mut m = Vec::new();
    if let Some(l) = &id.local {
        m.push(match l.user_id {
            Some(uid) => format!("local(uid={uid})"),
            None => "local(any)".to_string(),
        });
    }
    if id.basic.is_some() {
        m.push("basic".to_string());
    }
    if id.tls.is_some() {
        m.push("tls".to_string());
    }
    if m.is_empty() {
        "-".to_string()
    } else {
        m.join(",")
    }
}

/// `api`, or `layer <file>` for identities from layer files.
fn source(id: &Identity) -> String {
    match id.source {
        IdentitySource::Api => "api".to_string(),
        IdentitySource::Layer => format!("layer {}", id.layer.as_deref().unwrap_or("?")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vigil_types::identity::{BasicIdentity, TlsIdentity};

    fn id(source: IdentitySource, layer: Option<&str>) -> Identity {
        Identity {
            name: "log-relay".into(),
            access: IdentityAccess::Read,
            local: None,
            basic: None,
            tls: None,
            source,
            layer: layer.map(Into::into),
        }
    }

    #[test]
    fn auth_lists_every_method() {
        let mut i = id(IdentitySource::Api, None);
        assert_eq!(auth_methods(&i), "-");
        i.basic = Some(BasicIdentity {
            password_hash: "$6$x$y".into(),
        });
        assert_eq!(auth_methods(&i), "basic", "basic was shown as - before");
        i.local = Some(LocalIdentity {
            user_id: Some(1000),
        });
        i.tls = Some(TlsIdentity {
            ca_cert: String::new(),
        });
        assert_eq!(auth_methods(&i), "local(uid=1000),basic,tls");
    }

    #[test]
    fn source_names_the_layer_file() {
        assert_eq!(source(&id(IdentitySource::Api, None)), "api");
        assert_eq!(
            source(&id(IdentitySource::Layer, Some("001-auth"))),
            "layer 001-auth"
        );
    }
}
