//! CLI argument definitions for vigil-log-relay.

use std::path::PathBuf;

use clap::Parser;

mod connection;
mod kubernetes;
mod reconnect;
mod sink;
#[cfg(test)]
mod tests;

pub use connection::Connection;
pub use kubernetes::Kubernetes;
pub use reconnect::Reconnect;
pub use sink::Sink;

#[derive(Parser, Debug)]
#[command(
    name = "vigil-log-relay",
    version,
    about = "Read ndjson log streams from Kubernetes pods, HTTP endpoints, or Unix sockets and forward them to a TCP sink",
    next_help_heading = "Source"
)]
pub struct Cli {
    // ---- Source (exactly one required) ------------------------------------
    /// Watch Running pods via the Kubernetes API and stream their logs.
    /// Requires an in-cluster service account (KUBERNETES_SERVICE_HOST).
    #[arg(long, env = "SOURCE_KUBERNETES", help_heading = "Source")]
    pub kubernetes: bool,

    /// Read ndjson stream from an HTTP/HTTPS URL (cannot combine with --source-socket).
    #[arg(
        long,
        env = "SOURCE_URL",
        value_name = "URL",
        conflicts_with = "source_socket",
        help_heading = "Source"
    )]
    pub source_url: Option<String>,

    /// Read ndjson stream via a Unix-domain socket (cannot combine with --source-url).
    /// At most 107 bytes (Linux limit for Unix socket paths).
    #[arg(
        long,
        env = "SOURCE_SOCKET",
        value_name = "PATH",
        conflicts_with = "source_url",
        help_heading = "Source"
    )]
    pub source_socket: Option<PathBuf>,

    /// HTTP path to request over --source-socket.
    #[arg(
        long,
        env = "SOURCE_PATH",
        value_name = "PATH",
        default_value = "/v1/logs/follow?format=ndjson",
        help_heading = "Source"
    )]
    pub source_path: String,

    #[command(flatten)]
    pub conn: Connection,

    #[command(flatten)]
    pub reconnect: Reconnect,

    #[command(flatten)]
    pub k8s: Kubernetes,

    #[command(flatten)]
    pub sink: Sink,

    // ---- Health Check -----------------------------------------------------
    /// Address serving GET /healthz → 200 ok / 503 stale. Port 0 picks a
    /// free port (logged at startup).
    #[arg(
        long,
        env = "HEALTHCHECK",
        default_value = "127.0.0.1:9091",
        value_name = "HOST:PORT",
        help_heading = "Health Check"
    )]
    pub healthcheck: String,

    /// Seconds without a tick before /healthz returns 503 (≥ 3× --watch-interval).
    ///
    /// Kubernetes: tick per watch cycle.
    /// HTTP modes: background tick every 30 s.
    #[arg(
        long,
        env = "HEALTHCHECK_MAX_AGE",
        default_value = "30",
        value_name = "SECS",
        help_heading = "Health Check"
    )]
    pub healthcheck_max_age: u64,

    // ---- Logging ----------------------------------------------------------
    /// Format of the relay's own log.
    #[arg(
        long,
        env = "VIGIL_LOG_FORMAT",
        value_enum,
        default_value_t = LogFormat::Text,
        help_heading = "Logging"
    )]
    pub log_format: LogFormat,

    /// Enable debug logging with timestamps.
    #[arg(long, env = "RELAY_DEBUG", help_heading = "Logging")]
    pub debug: bool,

    // ---- Runtime ----------------------------------------------------------
    /// Worker threads of the async runtime. Default: 2, or 1 with a single
    /// CPU. Each costs about 2 MiB; raise it only when watching very many pods.
    #[arg(
        long,
        env = "VIGIL_LOG_WORKER_THREADS",
        value_name = "N",
        help_heading = "Runtime"
    )]
    pub worker_threads: Option<std::num::NonZeroUsize>,
}

impl Cli {
    /// Checks clap cannot express. `--kubernetes` and `--source-insecure` are
    /// flags that may come from `SOURCE_KUBERNETES=false` or
    /// `SOURCE_INSECURE=false`, which clap would count as "present", so their
    /// conflicts are checked here on the actual value.
    pub fn validate(&self) -> Result<(), String> {
        if self.kubernetes {
            let conn = &self.conn;
            let other = [
                (self.source_url.is_some(), "--source-url"),
                (self.source_socket.is_some(), "--source-socket"),
                (conn.source_cacert.is_some(), "--source-cacert"),
                (conn.source_user_file.is_some(), "--source-user-file"),
                (conn.source_cert.is_some(), "--source-cert"),
                (conn.source_key.is_some(), "--source-key"),
            ];
            if let Some((_, flag)) = other.iter().find(|(set, _)| *set) {
                return Err(format!("{flag} cannot be used with --kubernetes"));
            }
        }
        if self.conn.source_insecure && (self.kubernetes || self.source_socket.is_some()) {
            return Err(
                "--source-insecure only applies to --source-url; with --kubernetes \
                 configure TLS for the Kubernetes API in the kubeconfig"
                    .to_string(),
            );
        }
        Ok(())
    }
}

/// `--log-format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum LogFormat {
    Text,
    Json,
}
