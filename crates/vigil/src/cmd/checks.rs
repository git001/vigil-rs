// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use clap::Subcommand;
use vigil_client::client::VigilClient;

#[derive(Debug, Subcommand)]
pub enum ChecksCmd {
    /// List health checks and their current status.
    #[command(name = "list")]
    List {
        /// Filter by check name(s).
        names: Vec<String>,
    },
}

pub async fn run(client: &VigilClient, sub: ChecksCmd) -> anyhow::Result<()> {
    let ChecksCmd::List { names } = sub;
    let checks = client.list_checks(&names).await?;
    if checks.is_empty() {
        println!("No checks.");
        return Ok(());
    }
    println!(
        "{:<24} {:<8} {:<6} {:<10} Next run",
        "Check", "Level", "Status", "Failures"
    );
    println!("{}", "-".repeat(68));
    for chk in checks {
        let next = match chk.next_run_in_secs {
            None => "pending".to_string(),
            Some(s) if s < 60 => format!("{}s", s),
            Some(s) if s < 3600 => format!("{}m {}s", s / 60, s % 60),
            Some(s) => format!("{}h {}m", s / 3600, (s % 3600) / 60),
        };
        println!(
            "{:<24} {:<8} {:<6} {:<10} {}",
            chk.name,
            format!("{:?}", chk.level).to_lowercase(),
            format!("{:?}", chk.status).to_lowercase(),
            format!("{}/{}", chk.failures, chk.threshold),
            next,
        );
    }
    Ok(())
}
