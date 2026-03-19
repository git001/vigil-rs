// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod cmd;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use vigil_client::client::{HttpConfig, VigilClient};

// ---------------------------------------------------------------------------
// CLI arguments
// ---------------------------------------------------------------------------

#[derive(Debug, Parser)]
#[command(name = "vigil", version, about = "vigil service supervisor CLI")]
struct Args {
    /// Unix socket path of the vigild daemon.
    /// Ignored when --url is set.
    #[arg(long, env = "VIGIL_SOCKET", default_value = "/run/vigil/vigild.sock")]
    socket: PathBuf,

    /// HTTP or HTTPS base URL of the vigild daemon (e.g. https://host:8443).
    /// When set, --socket is ignored.
    #[arg(long, env = "VIGIL_URL")]
    url: Option<String>,

    /// Skip TLS certificate verification (useful for self-signed certs).
    /// Only effective with --url https://...
    #[arg(long, short = 'k')]
    insecure: bool,

    /// HTTP or HTTPS proxy URL.
    /// Falls back to HTTPS_PROXY / ALL_PROXY / HTTP_PROXY env vars.
    /// Ignored when using --socket (Unix transport).
    #[arg(long, env = "VIGIL_PROXY")]
    proxy: Option<String>,

    /// PEM file with one or more CA certificates for the proxy's TLS endpoint
    /// (e.g. corporate MITM proxy). Multiple certs may be concatenated.
    /// Ignored when using --socket (Unix transport).
    #[arg(long, env = "VIGIL_PROXY_CACERT")]
    proxy_cacert: Option<PathBuf>,

    /// Comma-separated hosts to bypass the proxy.
    /// "local.com" matches local.com, local.com:80, www.local.com
    /// but not www.notlocal.com.
    /// Ignored when using --socket (Unix transport).
    #[arg(long, env = "VIGIL_NO_PROXY")]
    no_proxy: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

// ---------------------------------------------------------------------------
// Command tree
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Show daemon info (version, boot-id, addresses).
    #[command(name = "system-info")]
    SystemInfo,

    /// Manage services.
    #[command(name = "services", subcommand)]
    Services(cmd::services::ServicesCmd),

    /// Manage health checks.
    #[command(name = "checks", subcommand)]
    Checks(cmd::checks::ChecksCmd),

    /// Show recent or live log output from services.
    #[command(name = "logs")]
    Logs {
        /// Filter by service name(s).
        services: Vec<String>,
        /// Number of lines to show (default 100).
        #[arg(short, long)]
        n: Option<usize>,
        /// Follow the log stream (like tail -f).
        #[arg(short, long)]
        follow: bool,
    },

    /// Reload layers from disk and apply changes.
    #[command(name = "replan")]
    Replan,

    /// Control the vigild daemon itself.
    #[command(name = "vigild", subcommand)]
    Vigild(cmd::vigild::VigilDCmd),

    /// Manage access identities.
    #[command(name = "identities", subcommand)]
    Identities(cmd::identities::IdentitiesCmd),
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let client = if let Some(url) = args.url {
        VigilClient::new_http(url, HttpConfig {
            insecure: args.insecure,
            proxy: args.proxy,
            proxy_cacert: args.proxy_cacert,
            no_proxy: args.no_proxy,
        })?
    } else {
        VigilClient::new_unix(args.socket)
    };

    match args.cmd {
        Cmd::SystemInfo => {
            let info = client.system_info().await?;
            println!("version:      {}", info.version);
            println!("boot-id:      {}", info.boot_id);
            println!("http-address: {}", info.http_address);
            if let Some(addr) = &info.https_address {
                println!("https-address: {}", addr);
            }
        }
        Cmd::Services(sub) => cmd::services::run(&client, sub).await?,
        Cmd::Checks(sub) => cmd::checks::run(&client, sub).await?,
        Cmd::Logs { services, n, follow } => cmd::logs::run(&client, &services, n, follow).await?,
        Cmd::Replan => {
            client.replan().await?;
            println!("Replan completed.");
        }
        Cmd::Vigild(sub) => cmd::vigild::run(&client, sub).await?,
        Cmd::Identities(sub) => cmd::identities::run(&client, sub).await?,
    }

    Ok(())
}
