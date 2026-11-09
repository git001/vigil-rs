// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The command line of `vigil`: global options and the command tree.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use vigil_client::client::{HttpConfig, VigilClient};

use crate::cmd;

// ---------------------------------------------------------------------------
// CLI arguments
// ---------------------------------------------------------------------------

#[derive(Debug, Parser)]
#[command(name = "vigil", version, about = "vigil service supervisor CLI")]
pub(crate) struct Args {
    #[arg(
        long,
        env = "VIGIL_PATH_PREFIX",
        default_value = vigil_types::paths::DEFAULT_PATH_PREFIX,
        help = "Runtime directory of vigild (same as vigild --path-prefix).\nThe socket is <path-prefix>/vigild.sock unless --socket is set.\nAt most 95 bytes, Unix socket paths are limited to 107."
    )]
    path_prefix: PathBuf,

    #[arg(
        long,
        env = "VIGIL_SOCKET",
        help = "Unix socket path of the vigild daemon.\nDefault: <path-prefix>/vigild.sock. Ignored when --url is set.\nAt most 107 bytes (Linux limit for Unix socket paths)."
    )]
    socket: Option<PathBuf>,

    #[arg(
        long,
        env = "VIGIL_SOCKET_UID",
        help = "Also trust a daemon socket owned by this UID.\nBy default only root or your own UID is accepted, so a socket\nplaced by another user is never talked to."
    )]
    socket_uid: Option<u32>,

    #[arg(
        long,
        env = "VIGIL_URL",
        help = "HTTP or HTTPS base URL of the vigild daemon\n(e.g. https://host:8443). When set, --socket is ignored."
    )]
    url: Option<String>,

    #[arg(
        long,
        short = 'k',
        env = "VIGIL_INSECURE",
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
    pub(crate) cmd: Cmd,
}

// ---------------------------------------------------------------------------
// Command tree
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
pub(crate) enum Cmd {
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
        #[arg(short = 'n', long, alias = "n", value_name = "N")]
        lines: Option<usize>,
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

impl Args {
    /// The client for `--url` (HTTP/HTTPS) or the Unix socket.
    pub(crate) fn client(&self) -> anyhow::Result<VigilClient> {
        let client = if let Some(url) = self.url.clone() {
            VigilClient::new_http(
                url,
                HttpConfig {
                    insecure: self.insecure,
                    user: self.user.clone(),
                    cert: self.cert.clone(),
                    key: self.key.clone(),
                    cacert: self.cacert.clone(),
                    proxy: self.proxy.clone(),
                    proxy_cacert: self.proxy_cacert.clone(),
                    no_proxy: self.no_proxy.clone(),
                },
            )?
        } else {
            VigilClient::new_unix_trusting(
                self.socket
                    .clone()
                    .unwrap_or_else(|| vigil_types::paths::socket_path(&self.path_prefix)),
                self.socket_uid,
            )
        };
        Ok(client)
    }
}

#[cfg(test)]
mod tests {
    use super::{Args, Cmd};
    use clap::Parser;

    fn lines(argv: &[&str]) -> Option<usize> {
        match Args::try_parse_from(argv).unwrap().cmd {
            Cmd::Logs { lines, .. } => lines,
            other => panic!("not logs: {other:?}"),
        }
    }

    #[test]
    fn logs_lines_has_short_long_and_old_alias() {
        assert_eq!(lines(&["vigil", "logs", "-n", "3"]), Some(3));
        assert_eq!(lines(&["vigil", "logs", "--lines", "4"]), Some(4));
        assert_eq!(lines(&["vigil", "logs", "--n", "5"]), Some(5));
        assert_eq!(lines(&["vigil", "logs"]), None);
    }

    #[test]
    fn identities_remove_needs_a_name() {
        assert!(Args::try_parse_from(["vigil", "identities", "remove"]).is_err());
        assert!(Args::try_parse_from(["vigil", "identities", "remove", "ops"]).is_ok());
    }
}
