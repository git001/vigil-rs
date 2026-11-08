//! How vigil-log-relay connects to its source: timeouts, keepalive, TLS
//! and proxy.

use std::path::PathBuf;

use clap::Args;

use crate::SourceConnConfig;
use crate::keepalive::Keepalive;

#[derive(Args, Debug)]
pub struct Connection {
    // ---- Source Connection ------------------------------------------------
    /// TCP connect timeout in ms (0 = no timeout).
    #[arg(
        long,
        env = "SOURCE_CONNECT_TIMEOUT",
        default_value = "10000",
        value_name = "MS",
        help_heading = "Source Connection"
    )]
    pub source_connect_timeout: u64,

    /// Max time without new data before triggering reconnect (0 = disabled).
    #[arg(
        long,
        env = "SOURCE_READ_TIMEOUT",
        default_value = "0",
        value_name = "MS",
        help_heading = "Source Connection"
    )]
    pub source_read_timeout: u64,

    /// Per-line idle timeout in ms; reconnects if no line arrives (0 = disabled).
    ///
    /// Useful to detect hung streams where the connection stays open but
    /// no data flows.
    #[arg(
        long,
        env = "SOURCE_IDLE_TIMEOUT",
        default_value = "0",
        value_name = "MS",
        help_heading = "Source Connection"
    )]
    pub source_idle_timeout: u64,

    /// TCP keepalive: seconds between probes (0 = off); 3 unanswered probes
    /// close the connection.
    #[arg(
        long,
        env = "SOURCE_KEEPALIVE_INTERVAL",
        default_value = "0",
        value_name = "SECS",
        help_heading = "Source Connection"
    )]
    pub source_keepalive_interval: u64,

    /// TCP keepalive: seconds of silence before the first probe
    /// (0 = 3 x --source-keepalive-interval).
    ///
    /// Only applied when --source-keepalive-interval > 0.
    #[arg(
        long,
        env = "SOURCE_KEEPALIVE_TIMEOUT",
        default_value = "0",
        value_name = "SECS",
        help_heading = "Source Connection"
    )]
    pub source_keepalive_timeout: u64,

    /// Skip TLS certificate verification for the source connection.
    ///
    /// Only for --source-url (rejected with --kubernetes: configure TLS for the
    /// Kubernetes API in the kubeconfig). Useful when vigild uses a self-signed certificate.
    #[arg(long, env = "SOURCE_INSECURE", help_heading = "Source Connection")]
    pub source_insecure: bool,

    /// PEM file with one or more CA certificates (chain) to verify the source's TLS.
    ///
    /// Supports chain files with multiple concatenated PEM blocks.
    /// Only for --source-url (with --kubernetes, the CA comes from the
    /// ServiceAccount or the kubeconfig).
    #[arg(
        long,
        env = "SOURCE_CACERT",
        conflicts_with = "source_socket",
        value_name = "PATH",
        help_heading = "Source Connection"
    )]
    pub source_cacert: Option<PathBuf>,

    /// File with HTTP Basic Auth credentials `user:password` for the source.
    ///
    /// Read on every (re)connect, so a rotated Secret is picked up without a
    /// restart. A trailing newline is ignored. Use this with a vigild
    /// identity of access `read` to stream logs over vigild's TLS API.
    /// Only for --source-url (with --kubernetes, authentication comes from the
    /// ServiceAccount token or the kubeconfig).
    #[arg(
        long,
        env = "SOURCE_USER_FILE",
        conflicts_with = "source_socket",
        value_name = "PATH",
        help_heading = "Source Connection"
    )]
    pub source_user_file: Option<PathBuf>,

    /// PEM client certificate for mTLS to the source (requires --source-key).
    ///
    /// Only for --source-url. For the Kubernetes API, put the client
    /// certificate into the kubeconfig (`client-certificate`/`client-key`).
    #[arg(
        long,
        env = "SOURCE_CERT",
        conflicts_with = "source_socket",
        value_name = "PATH",
        requires = "source_key",
        help_heading = "Source Connection"
    )]
    pub source_cert: Option<PathBuf>,

    /// PEM private key matching --source-cert.
    ///
    /// Only for --source-url.
    #[arg(
        long,
        env = "SOURCE_KEY",
        conflicts_with = "source_socket",
        value_name = "PATH",
        requires = "source_cert",
        help_heading = "Source Connection"
    )]
    pub source_key: Option<PathBuf>,

    /// Proxy URL for HTTP/HTTPS source (overrides HTTP_PROXY / HTTPS_PROXY env vars).
    ///
    /// Applies to --source-url and --kubernetes modes.
    #[arg(
        long,
        env = "SOURCE_PROXY",
        value_name = "URL",
        help_heading = "Source Connection"
    )]
    pub source_proxy: Option<String>,

    /// Skip TLS certificate verification for the proxy connection.
    ///
    /// Applies to --source-url and --kubernetes modes.
    #[arg(
        long,
        env = "SOURCE_PROXY_INSECURE",
        help_heading = "Source Connection"
    )]
    pub source_proxy_insecure: bool,

    /// PEM file with one or more CA certificates (chain) to verify the proxy's TLS.
    ///
    /// Supports chain files with multiple concatenated PEM blocks.
    /// Applies to --source-url and --kubernetes modes.
    #[arg(
        long,
        env = "SOURCE_PROXY_CACERT",
        value_name = "PATH",
        help_heading = "Source Connection"
    )]
    pub source_proxy_cacert: Option<PathBuf>,

    /// Comma-separated list of hosts/CIDRs that bypass the source proxy.
    ///
    /// Each entry is matched against the request host (port stripped).
    /// Supports exact hostnames (`internal.corp`), domain suffixes
    /// (`.corp` matches `api.corp`), IPv4 CIDRs (`192.168.0.0/16`),
    /// and IPv6 CIDRs (`fd00::/8`).
    /// Only for --source-url; with --kubernetes it has no effect (there is
    /// one API server: leave --source-proxy unset to connect directly).
    #[arg(
        long,
        env = "SOURCE_NO_PROXY",
        value_name = "LIST",
        help_heading = "Source Connection"
    )]
    pub source_no_proxy: Option<String>,
}

impl Connection {
    /// The connection settings of `--source-url` and `--source-socket`.
    pub fn source_conn_config(&self) -> SourceConnConfig {
        SourceConnConfig {
            connect_timeout_ms: self.source_connect_timeout,
            read_timeout_ms: self.source_read_timeout,
            idle_timeout_ms: self.source_idle_timeout,
            keepalive: Keepalive {
                interval_secs: self.source_keepalive_interval,
                timeout_secs: self.source_keepalive_timeout,
            },
            source_insecure: self.source_insecure,
            source_cacert: self.source_cacert.clone(),
            proxy_url: self.source_proxy.clone(),
            proxy_insecure: self.source_proxy_insecure,
            proxy_cacert: self.source_proxy_cacert.clone(),
            no_proxy: self.source_no_proxy.clone(),
            source_user_file: self.source_user_file.clone(),
            source_cert: self.source_cert.clone(),
            source_key: self.source_key.clone(),
        }
    }
}
