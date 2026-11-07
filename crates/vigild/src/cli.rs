// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! vigild's command line (binary only, not part of the library).

use std::path::PathBuf;

use clap::Parser;
use vigil_types::identity::IdentityAccess;
use vigild::service::ExitCodeMode;
use vigild::{logs, path_prefix};

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum TlsAnonymous {
    Open,
    Metrics,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum ExitCode {
    Propagate,
    Fixed,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum LogFormat {
    Text,
    Json,
}

#[derive(Debug, Parser)]
#[command(name = "vigild", version, about, long_about = concat!(
    "vigild — Rust service supervisor daemon\n",
    "\n",
    "Reads layer YAML files from --layers-dir, merges them into a plan, and\n",
    "supervises the declared services and health checks.\n",
    "\n",
    "HTTP API on the Unix socket (<path-prefix>/vigild.sock) and, with\n",
    "--tls-addr, over HTTPS. A running vigild describes it at /docs (Swagger\n",
    "UI) and /openapi.json; see also docs/api-reference/rest-api.md.\n",
    "\n",
    "Use 'vigil --help' for the command-line client.",
))]
pub(crate) struct Args {
    /// Directory containing layer YAML files.
    #[arg(long, env = "VIGIL_LAYERS", default_value = "/etc/vigil/layers")]
    pub(crate) layers_dir: PathBuf,

    /// Directory for everything vigild creates at runtime (control socket,
    /// sd_notify sockets). Checked at startup: must be a directory owned by
    /// vigild's UID, not a symlink, not writable by others. A missing last
    /// component is created (mode 0711). On OpenShift use e.g. /tmp/vigil.
    #[arg(long, env = "VIGIL_PATH_PREFIX", default_value = path_prefix::DEFAULT_PATH_PREFIX)]
    pub(crate) path_prefix: PathBuf,

    /// Unix socket path for the HTTP API. Default: <path-prefix>/vigild.sock.
    #[arg(long, env = "VIGIL_SOCKET")]
    pub(crate) socket: Option<PathBuf>,

    /// Permission bits of the Unix socket (octal). Default 600: owner only.
    /// Widen (e.g. 660 or 666) only together with local identities, because
    /// without identities every caller that can connect is admin.
    #[arg(long, env = "VIGIL_SOCKET_MODE", default_value = "600", value_parser = parse_octal_mode)]
    pub(crate) socket_mode: u32,

    /// Address for the HTTPS API (e.g. "0.0.0.0:8443"). Disabled if not set.
    #[arg(long, env = "VIGIL_TLS_ADDR")]
    pub(crate) tls_addr: Option<String>,

    /// PEM certificate file for TLS. Auto-generates self-signed if omitted.
    #[arg(long, env = "VIGIL_CERT")]
    pub(crate) cert: Option<PathBuf>,

    /// PEM private key file for TLS. Auto-generates self-signed if omitted.
    #[arg(long, env = "VIGIL_KEY")]
    pub(crate) key: Option<PathBuf>,

    /// PEM file with one or more CA certificates used to verify TLS client
    /// certificates (mTLS).  Supports chain files with multiple concatenated
    /// PEM blocks.  Client certificates are **optional** — connections without
    /// a client cert still work and fall back to Basic Auth or local UID auth.
    /// Has no effect unless `--tls-addr` is also set.
    #[arg(long, env = "VIGIL_TLS_CLIENT_CA")]
    pub(crate) tls_client_ca: Option<PathBuf>,

    /// Access of TLS callers without a matching identity: `open` (only
    /// /v1/checks, /v1/system-info, API docs) or `metrics` (also
    /// /v1/metrics). Logs, services and anything that changes state always
    /// need an identity over TLS.
    #[arg(long, env = "VIGIL_TLS_ANONYMOUS_ACCESS", value_enum, default_value_t = TlsAnonymous::Metrics)]
    pub(crate) tls_anonymous_access: TlsAnonymous,

    /// vigild's exit code when a policy shuts it down: `propagate` (the
    /// service's code after exit-code-map, 128+signal for signals) or `fixed`
    /// (v3 behaviour: 0 on success, 10 on failure).
    #[arg(long, env = "VIGIL_EXIT_CODE", value_enum, default_value_t = ExitCode::Propagate)]
    pub(crate) exit_code: ExitCode,

    /// Enable init/subreaper mode: reap orphaned zombie processes.
    /// Automatically active when vigild runs as PID 1.
    #[arg(long, env = "VIGIL_REAPER")]
    pub(crate) reaper: bool,

    /// Format of vigild's own log.
    #[arg(long, env = "VIGIL_LOG_FORMAT", value_enum, default_value_t = LogFormat::Text)]
    pub(crate) log_format: LogFormat,

    /// Reload the layers automatically when the files in --layers-dir
    /// change (e.g. a mounted ConfigMap), checking every INTERVAL
    /// (1s to 24h, e.g. 30s). Off by default.
    #[arg(long, env = "VIGIL_WATCH_LAYERS", value_name = "INTERVAL", value_parser = vigild::duration::parse_interval)]
    pub(crate) watch_layers: Option<std::time::Duration>,

    /// Per-service log ring-buffer size (number of lines kept in memory).
    /// Also controls the SSE broadcast channel depth (half this value,
    /// clamped to [64, 4096]), which determines how many entries can queue
    /// for a slow log-stream consumer before it starts skipping.
    #[arg(long, env = "VIGIL_LOG_BUFFER", default_value_t = logs::DEFAULT_BUFFER_CAPACITY)]
    pub(crate) log_buffer: usize,
}

impl Args {
    /// `--exit-code` as the overlord takes it.
    pub(crate) fn exit_code_mode(&self) -> ExitCodeMode {
        match self.exit_code {
            ExitCode::Fixed => ExitCodeMode::Fixed,
            ExitCode::Propagate => ExitCodeMode::Propagate,
        }
    }

    /// `--tls-anonymous-access` as the API takes it.
    pub(crate) fn anonymous_access(&self) -> IdentityAccess {
        match self.tls_anonymous_access {
            TlsAnonymous::Open => IdentityAccess::Open,
            TlsAnonymous::Metrics => IdentityAccess::Metrics,
        }
    }
}

/// `--socket-mode`: octal permission bits, with or without `0o`, at most 777.
fn parse_octal_mode(s: &str) -> Result<u32, String> {
    let digits = s.strip_prefix("0o").unwrap_or(s);
    let mode =
        u32::from_str_radix(digits, 8).map_err(|e| format!("invalid octal mode {s:?}: {e}"))?;
    if mode > 0o777 {
        return Err(format!("mode {s:?} out of range (max 777)"));
    }
    Ok(mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_mode_with_and_without_prefix() {
        assert_eq!(parse_octal_mode("600"), Ok(0o600));
        assert_eq!(parse_octal_mode("0o660"), Ok(0o660));
        assert_eq!(parse_octal_mode("0777"), Ok(0o777));
    }

    #[test]
    fn octal_mode_rejects_bad_digits_and_range() {
        assert!(
            parse_octal_mode("680")
                .unwrap_err()
                .contains("invalid octal mode")
        );
        assert!(
            parse_octal_mode("1000")
                .unwrap_err()
                .contains("out of range")
        );
        assert!(parse_octal_mode("").is_err());
    }

    #[test]
    fn defaults_and_mappings() {
        let args = Args::parse_from(["vigild"]);
        assert_eq!(args.socket_mode, 0o600);
        assert_eq!(args.exit_code_mode(), ExitCodeMode::Propagate);
        assert_eq!(args.anonymous_access(), IdentityAccess::Metrics);

        let args = Args::parse_from([
            "vigild",
            "--exit-code",
            "fixed",
            "--tls-anonymous-access",
            "open",
        ]);
        assert_eq!(args.exit_code_mode(), ExitCodeMode::Fixed);
        assert_eq!(args.anonymous_access(), IdentityAccess::Open);
    }
}
