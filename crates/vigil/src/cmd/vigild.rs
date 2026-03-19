// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use clap::Subcommand;
use vigil_client::client::VigilClient;
use vigil_types::api::DaemonAction;

#[derive(Debug, Subcommand)]
pub enum VigilDCmd {
    /// Show daemon status (version, uptime, addresses).
    #[command(name = "status")]
    Status,

    /// Gracefully stop the daemon and all supervised services.
    #[command(name = "stop")]
    Stop,

    /// Gracefully stop all services and re-execute the daemon in-place.
    #[command(name = "restart")]
    Restart,
}

pub async fn run(client: &VigilClient, sub: VigilDCmd) -> anyhow::Result<()> {
    match sub {
        VigilDCmd::Status => {
            let info = client.system_info().await?;
            let uptime = chrono::Utc::now()
                .signed_duration_since(info.start_time)
                .to_std()
                .unwrap_or_default();
            let s = uptime.as_secs();
            println!("version:      {}", info.version);
            println!("boot-id:      {}", info.boot_id);
            println!("uptime:       {}d {}h {}m {}s", s / 86400, (s % 86400) / 3600, (s % 3600) / 60, s % 60);
            println!("http-address: {}", info.http_address);
            if let Some(addr) = &info.https_address {
                println!("https-address: {}", addr);
            }
        }
        VigilDCmd::Stop => {
            client.daemon_action(DaemonAction::Stop).await?;
            println!("Daemon stop initiated.");
        }
        VigilDCmd::Restart => {
            client.daemon_action(DaemonAction::Restart).await?;
            println!("Daemon restart initiated.");
        }
    }
    Ok(())
}
