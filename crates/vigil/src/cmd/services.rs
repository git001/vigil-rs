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
        /// Output format.
        #[arg(short = 'o', long, env = "VIGIL_OUTPUT", default_value = "table", value_parser = ["table", "json"])]
        output: String,
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
        ServicesCmd::List { names, output } => {
            let services = client.list_services(&names).await?;
            if output == "json" {
                println!("{}", serde_json::to_string_pretty(&services)?);
                return Ok(());
            }
            if services.is_empty() {
                println!("No services.");
                return Ok(());
            }
            // The sd_notify STATUS= column only appears when a service set one.
            let with_msg = services.iter().any(|s| s.status_message.is_some());
            let header = format!(
                "{:<24} {:<10} {:<10} {:<18} {:<18} {:<12} {:<10}",
                "Service",
                "Startup",
                "Status",
                "On-Success",
                "On-Failure",
                "Stop-Signal",
                "Last-Exit"
            );
            if with_msg {
                println!("{header} Message");
            } else {
                println!("{header}");
            }
            println!("{}", "-".repeat(108));
            for svc in services {
                let row = format!(
                    "{:<24} {:<10} {:<10} {:<18} {:<18} {:<12} {:<10}",
                    svc.name,
                    format!("{:?}", svc.startup).to_lowercase(),
                    format!("{:?}", svc.current).to_lowercase(),
                    svc.on_success,
                    svc.on_failure,
                    svc.stop_signal,
                    fmt_last_exit(svc.last_exit),
                );
                match (&svc.status_message, with_msg) {
                    (Some(m), _) => println!("{row} {m}"),
                    (None, true) => println!("{row} -"),
                    (None, false) => println!("{row}"),
                }
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
                &client
                    .services_action(ServiceAction::Restart, names)
                    .await?,
            );
        }
    }
    Ok(())
}

/// `3`, `SIGTERM`, or `-` if the service has not exited yet.
fn fmt_last_exit(exit: Option<vigil_types::api::ExitInfo>) -> String {
    match exit {
        Some(e) => match (e.code, e.signal) {
            (Some(c), _) => c.to_string(),
            (None, Some(s)) => nix::sys::signal::Signal::try_from(s)
                .map(|sig| sig.as_str().to_string())
                .unwrap_or_else(|_| format!("signal {s}")),
            (None, None) => "?".into(),
        },
        None => "-".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::fmt_last_exit;
    use vigil_types::api::ExitInfo;

    #[test]
    fn last_exit_formatting() {
        assert_eq!(fmt_last_exit(None), "-");
        assert_eq!(
            fmt_last_exit(Some(ExitInfo {
                code: Some(3),
                signal: None
            })),
            "3"
        );
        assert_eq!(
            fmt_last_exit(Some(ExitInfo {
                code: None,
                signal: Some(15)
            })),
            "SIGTERM"
        );
        assert_eq!(
            fmt_last_exit(Some(ExitInfo {
                code: None,
                signal: None
            })),
            "?"
        );
    }
}
