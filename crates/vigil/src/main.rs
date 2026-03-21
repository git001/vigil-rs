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
    #[arg(
        long,
        env = "VIGIL_SOCKET",
        default_value = "/run/vigil/vigild.sock",
        help = "Unix socket path of the vigild daemon.\nIgnored when --url is set."
    )]
    socket: PathBuf,

    #[arg(
        long,
        env = "VIGIL_URL",
        help = "HTTP or HTTPS base URL of the vigild daemon\n(e.g. https://host:8443). When set, --socket is ignored."
    )]
    url: Option<String>,

    #[arg(
        long,
        short = 'k',
        help = "Skip TLS certificate verification (useful for self-signed certs).\nOnly effective with --url https://..."
    )]
    insecure: bool,

    #[arg(
        long,
        short = 'u',
        env = "VIGIL_USER",
        help = "HTTP Basic Auth credentials as 'username:password'.\nOnly effective with --url."
    )]
    user: Option<String>,

    #[arg(
        long,
        env = "VIGIL_CERT",
        help = "PEM file with a client certificate for mTLS.\nMust be used with --key. Only effective with --url https://..."
    )]
    cert: Option<PathBuf>,

    #[arg(
        long,
        env = "VIGIL_KEY",
        help = "PEM file with the private key for --cert.\nOnly effective with --url https://..."
    )]
    key: Option<PathBuf>,

    #[arg(
        long,
        env = "VIGIL_CACERT",
        help = "PEM file with CA certificates to trust for the vigild server.\nSupports chains (multiple concatenated PEM blocks).\nOnly effective with --url https://..."
    )]
    cacert: Option<PathBuf>,

    #[arg(
        long,
        env = "VIGIL_PROXY",
        help = "HTTP or HTTPS proxy URL.\nFalls back to HTTPS_PROXY / ALL_PROXY / HTTP_PROXY env vars.\nIgnored when using --socket (Unix transport)."
    )]
    proxy: Option<String>,

    #[arg(
        long,
        env = "VIGIL_PROXY_CACERT",
        help = "PEM file with CA certificates for the proxy TLS endpoint\n(e.g. corporate MITM proxy). Multiple certs may be concatenated.\nIgnored when using --socket (Unix transport)."
    )]
    proxy_cacert: Option<PathBuf>,

    #[arg(
        long,
        env = "VIGIL_NO_PROXY",
        help = "Comma-separated hosts to bypass the proxy.\n\"local.com\" matches local.com, local.com:80, www.local.com\nbut not www.notlocal.com.\nIgnored when using --socket (Unix transport)."
    )]
    no_proxy: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

// ---------------------------------------------------------------------------
// Command tree
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
enum Cmd {
    /// List alert configurations and their check status.
    #[command(name = "alerts", subcommand)]
    Alerts(cmd::alerts::AlertsCmd),

    /// Manage health checks.
    #[command(name = "checks", subcommand)]
    Checks(cmd::checks::ChecksCmd),

    /// Manage access identities.
    #[command(name = "identities", subcommand)]
    Identities(cmd::identities::IdentitiesCmd),

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

    /// Manage services.
    #[command(name = "services", subcommand)]
    Services(cmd::services::ServicesCmd),

    /// Show daemon info (version, boot-id, addresses).
    #[command(name = "system-info")]
    SystemInfo,

    /// Control the vigild daemon itself.
    #[command(name = "vigild", subcommand)]
    Vigild(cmd::vigild::VigilDCmd),
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let client = if let Some(url) = args.url {
        VigilClient::new_http(
            url,
            HttpConfig {
                insecure: args.insecure,
                user: args.user,
                cert: args.cert,
                key: args.key,
                cacert: args.cacert,
                proxy: args.proxy,
                proxy_cacert: args.proxy_cacert,
                no_proxy: args.no_proxy,
            },
        )?
    } else {
        VigilClient::new_unix(args.socket)
    };

    match args.cmd {
        Cmd::Alerts(sub) => cmd::alerts::run(&client, sub).await?,
        Cmd::Checks(sub) => cmd::checks::run(&client, sub).await?,
        Cmd::Identities(sub) => cmd::identities::run(&client, sub).await?,
        Cmd::Logs {
            services,
            n,
            follow,
        } => cmd::logs::run(&client, &services, n, follow).await?,
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
