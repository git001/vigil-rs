// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use clap::Subcommand;
use vigil_client::client::VigilClient;
use vigil_types;

#[derive(Debug, Subcommand)]
pub enum AlertsCmd {
    /// List alert configurations and their current check status.
    #[command(name = "list")]
    List {
        /// Filter by alert name(s).
        names: Vec<String>,
    },
}

pub async fn run(client: &VigilClient, sub: AlertsCmd) -> anyhow::Result<()> {
    let AlertsCmd::List { names } = sub;
    let alerts = client.list_alerts(&names).await?;
    if alerts.is_empty() {
        println!("No alerts.");
        return Ok(());
    }
    println!("{:<24} {:<14} {:<8} Checks", "Alert", "Format", "Status");
    println!("{}", "-".repeat(70));
    for alert in alerts {
        // Overall status: down if any watched check is down, unknown if none
        // observed yet, otherwise up.
        let overall = if alert
            .check_status
            .iter()
            .any(|cs| cs.status == Some(vigil_types::api::CheckStatus::Down))
        {
            "down"
        } else if alert.check_status.iter().all(|cs| cs.status.is_none()) {
            "unknown"
        } else {
            "up"
        };
        println!(
            "{:<24} {:<14} {:<8} {}",
            alert.name,
            format!("{:?}", alert.format).to_lowercase(),
            overall,
            alert.on_check.join(", "),
        );
    }
    Ok(())
}
