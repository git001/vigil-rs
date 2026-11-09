// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod cli;
mod cmd;

use clap::Parser;

use cli::{Args, Cmd};

// A single thread: the CLI sends one request (or follows one stream) and exits.
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let client = args.client()?;

    match args.cmd {
        Cmd::Alerts(sub) => cmd::alerts::run(&client, sub).await?,
        Cmd::Checks(sub) => cmd::checks::run(&client, sub).await?,
        Cmd::Identities(sub) => cmd::identities::run(&client, sub).await?,
        Cmd::Logs {
            services,
            lines,
            follow,
        } => cmd::logs::run(&client, &services, lines, follow).await?,
        Cmd::Plan => {
            let plan = client.get_plan().await?;
            print!("{}", serde_yaml::to_string(&plan)?);
        }
        Cmd::Replan => {
            client.replan().await?;
            println!("Replan completed.");
        }
        Cmd::Services(sub) => cmd::services::run(&client, sub).await?,
        Cmd::SystemInfo => {
            let info = client.system_info().await?;
            println!("version:      {}", info.version);
            println!("boot-id:      {}", info.boot_id);
            println!("http-address: {}", info.http_address);
            if let Some(addr) = &info.https_address {
                println!("https-address: {}", addr);
            }
        }
        Cmd::Vigild(sub) => cmd::vigild::run(&client, sub).await?,
    }

    Ok(())
}
