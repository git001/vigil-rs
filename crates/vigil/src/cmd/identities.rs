// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::collections::HashMap;

use clap::Subcommand;
use vigil_client::client::VigilClient;
use vigil_types::identity::{IdentityAccess, IdentitySpec, LocalIdentity};

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
    Remove { names: Vec<String> },
}

pub async fn run(client: &VigilClient, sub: IdentitiesCmd) -> anyhow::Result<()> {
    match sub {
        IdentitiesCmd::List { names } => {
            let ids = client.list_identities(&names).await?;
            if ids.is_empty() {
                println!("No identities.");
                return Ok(());
            }
            println!("{:<24} {:<8} {:<12}", "Name", "Access", "Auth");
            println!("{}", "-".repeat(46));
            for id in ids {
                let auth = match (&id.local, &id.tls) {
                    (Some(l), _) => match l.user_id {
                        Some(uid) => format!("local(uid={})", uid),
                        None => "local(any)".to_string(),
                    },
                    (_, Some(_)) => "tls".to_string(),
                    _ => "-".to_string(),
                };
                println!(
                    "{:<24} {:<8} {}",
                    id.name,
                    format!("{:?}", id.access).to_lowercase(),
                    auth,
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
