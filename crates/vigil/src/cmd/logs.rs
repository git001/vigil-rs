// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use vigil_client::client::VigilClient;

pub async fn run(
    client: &VigilClient,
    services: &[String],
    n: Option<usize>,
    follow: bool,
) -> anyhow::Result<()> {
    let entries = client.list_logs(services, n).await?;
    if entries.is_empty() && !follow {
        println!("No log entries.");
        return Ok(());
    }
    for e in entries {
        println!(
            "{} [{}] [{}] {}",
            e.timestamp.format("%Y-%m-%d %H:%M:%S%.3f"),
            e.service,
            format!("{:?}", e.stream).to_lowercase(),
            e.message,
        );
    }

    if follow {
        tokio::select! {
            res = client.follow_logs(services) => res?,
            _ = tokio::signal::ctrl_c() => {},
        }
    }
    Ok(())
}
