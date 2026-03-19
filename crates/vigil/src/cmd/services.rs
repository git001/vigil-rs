// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use clap::Subcommand;
use vigil_client::client::VigilClient;
use vigil_types::api::ServiceAction;

#[derive(Debug, Subcommand)]
pub enum ServicesCmd {
    /// List services and their current status.
    #[command(name = "list")]
    List {
        /// Filter by service name(s).
        names: Vec<String>,
    },

    /// Start one or more services (empty = all).
    #[command(name = "start")]
    Start { names: Vec<String> },

    /// Stop one or more services.
    #[command(name = "stop")]
    Stop { names: Vec<String> },

    /// Restart one or more services.
    #[command(name = "restart")]
    Restart { names: Vec<String> },
}

pub async fn run(client: &VigilClient, sub: ServicesCmd) -> anyhow::Result<()> {
    match sub {
        ServicesCmd::List { names } => {
            let services = client.list_services(&names).await?;
            if services.is_empty() {
                println!("No services.");
                return Ok(());
            }
            println!(
                "{:<24} {:<10} {:<10} {:<18} {:<18} {:<10}",
                "Service", "Startup", "Status", "On-Success", "On-Failure", "Stop-Signal"
            );
            println!("{}", "-".repeat(92));
            for svc in services {
                println!(
                    "{:<24} {:<10} {:<10} {:<18} {:<18} {:<10}",
                    svc.name,
                    format!("{:?}", svc.startup).to_lowercase(),
                    format!("{:?}", svc.current).to_lowercase(),
                    svc.on_success,
                    svc.on_failure,
                    svc.stop_signal,
                );
            }
        }

        ServicesCmd::Start { names } => {
            crate::cmd::print_change(&client.services_action(ServiceAction::Start, names).await?);
        }
        ServicesCmd::Stop { names } => {
            crate::cmd::print_change(&client.services_action(ServiceAction::Stop, names).await?);
        }
        ServicesCmd::Restart { names } => {
            crate::cmd::print_change(
                &client.services_action(ServiceAction::Restart, names).await?,
            );
        }
    }
    Ok(())
}
