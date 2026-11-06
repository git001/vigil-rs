// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use clap::Subcommand;
use vigil_client::client::VigilClient;
use vigil_types::api::ServiceAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Output {
    Table,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum ServicesCmd {
    /// List services and their current status.
    #[command(name = "list")]
    List {
        /// Filter by service name(s).
        names: Vec<String>,
        /// Output format.
        #[arg(short = 'o', long, env = "VIGIL_OUTPUT", value_enum, default_value_t = Output::Table)]
        output: Output,
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
            if output == Output::Json {
                println!("{}", serde_json::to_string_pretty(&services)?);
                return Ok(());
            }
            if services.is_empty() {
                println!("No services.");
                return Ok(());
            }
            // Optional columns only appear when a service has a value:
            // Next-Run (schedule) and Message (sd_notify STATUS=).
            let with_next = services.iter().any(|s| s.next_run.is_some());
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
            let mut header = header;
            if with_next {
                header.push_str(&format!(" {:<20}", "Next-Run"));
            }
            if with_msg {
                header.push_str(" Message");
            }
            println!("{}", header.trim_end());
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
                let mut row = row;
                if with_next {
                    let next = svc
                        .next_run
                        .map(|t| t.format("%Y-%m-%d %H:%M:%SZ").to_string())
                        .unwrap_or_else(|| "-".into());
                    row.push_str(&format!(" {next:<20}"));
                }
                if with_msg {
                    row.push(' ');
                    row.push_str(svc.status_message.as_deref().unwrap_or("-"));
                }
                println!("{}", row.trim_end());
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
