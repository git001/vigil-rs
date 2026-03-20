//! CLI argument definitions for vigil-log-relay.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name    = "vigil-log-relay",
    version,
    about   = "Read ndjson log streams from Kubernetes pods, HTTP endpoints, or Unix sockets and forward them to a TCP sink",
    next_help_heading = "Source"
)]
pub struct Cli {
    // ---- Source (exactly one required) ------------------------------------

    /// Watch Running pods via the Kubernetes API and stream their logs.
    /// Requires an in-cluster service account (KUBERNETES_SERVICE_HOST).
    #[arg(long, help_heading = "Source")]
    pub kubernetes: bool,

    /// Read ndjson stream from an HTTP/HTTPS URL (cannot combine with --source-socket).
    #[arg(long, value_name = "URL",
          conflicts_with_all = ["kubernetes", "source_socket"],
          help_heading = "Source")]
    pub source_url: Option<String>,

    /// Read ndjson stream via a Unix-domain socket (cannot combine with --source-url).
    #[arg(long, value_name = "PATH",
          conflicts_with_all = ["kubernetes", "source_url"],
          help_heading = "Source")]
    pub source_socket: Option<PathBuf>,

    /// HTTP path to request over --source-socket.
    #[arg(long, value_name = "PATH",
          default_value = "/v1/logs/follow?format=ndjson",
          help_heading = "Source")]
    pub source_path: String,

    // ---- Source Connection ------------------------------------------------

    /// TCP connect timeout in ms (0 = no timeout).
    #[arg(long, env = "SOURCE_CONNECT_TIMEOUT", default_value = "10000",
          value_name = "MS", help_heading = "Source Connection")]
    pub source_connect_timeout: u64,

    /// Max time without new data before triggering reconnect (0 = disabled).
    #[arg(long, env = "SOURCE_READ_TIMEOUT", default_value = "0",
          value_name = "MS", help_heading = "Source Connection")]
    pub source_read_timeout: u64,

    /// Per-line idle timeout in ms; reconnects if no line arrives (0 = disabled).
    ///
    /// Useful to detect hung streams where the connection stays open but
    /// no data flows.
    #[arg(long, env = "SOURCE_IDLE_TIMEOUT", default_value = "0",
          value_name = "MS", help_heading = "Source Connection")]
    pub source_idle_timeout: u64,

    /// TCP keepalive interval in seconds (0 = disabled).
    #[arg(long, env = "SOURCE_KEEPALIVE_INTERVAL", default_value = "0",
          value_name = "SECS", help_heading = "Source Connection")]
    pub source_keepalive_interval: u64,

    /// TCP keepalive probe timeout in seconds (0 = OS default).
    ///
    /// Only applied when --source-keepalive-interval > 0.
    #[arg(long, env = "SOURCE_KEEPALIVE_TIMEOUT", default_value = "0",
          value_name = "SECS", help_heading = "Source Connection")]
    pub source_keepalive_timeout: u64,

    /// Proxy URL for HTTP/HTTPS source (overrides HTTP_PROXY / HTTPS_PROXY env vars).
    ///
    /// Applies to --source-url and --kubernetes modes.
    #[arg(long, env = "SOURCE_PROXY", value_name = "URL",
          help_heading = "Source Connection")]
    pub source_proxy: Option<String>,

    /// Skip TLS certificate verification for the proxy connection.
    ///
    /// Applies to --source-url and --kubernetes modes.
    #[arg(long, env = "SOURCE_PROXY_INSECURE", help_heading = "Source Connection")]
    pub source_proxy_insecure: bool,

    /// PEM file with one or more CA certificates (chain) to verify the proxy's TLS.
    ///
    /// Supports chain files with multiple concatenated PEM blocks.
    /// Applies to --source-url and --kubernetes modes.
    #[arg(long, env = "SOURCE_PROXY_CACERT", value_name = "PATH",
          help_heading = "Source Connection")]
    pub source_proxy_cacert: Option<PathBuf>,

    // ---- Source Reconnect -------------------------------------------------

    /// Initial delay in ms; doubles each retry, capped at --source-reconnect-max.
    ///
    /// Triggers: connection refused, timeout, HTTP non-2xx, read error.
    /// Clean stream EOF resets the counter and delay.
    #[arg(long, env = "SOURCE_RECONNECT_DELAY", default_value = "500",
          value_name = "MS", help_heading = "Source Reconnect")]
    pub source_reconnect_delay: u64,

    /// Backoff ceiling in ms.
    #[arg(long, env = "SOURCE_RECONNECT_MAX", default_value = "30000",
          value_name = "MS", help_heading = "Source Reconnect")]
    pub source_reconnect_max: u64,

    /// Max consecutive failures before exit (0 = unlimited).
    ///
    /// vigild then restarts the process via on-failure: restart.
    /// Clean EOF does not count.
    #[arg(long, env = "SOURCE_RECONNECT_RETRIES", default_value = "0",
          value_name = "N", help_heading = "Source Reconnect")]
    pub source_reconnect_retries: u64,

    // ---- Kubernetes -------------------------------------------------------

    /// Namespace to watch.
    #[arg(long, env = "NAMESPACE", default_value = "default",
          help_heading = "Kubernetes")]
    pub namespace: String,

    /// Label selector, e.g. "app=myapp".
    #[arg(long, env = "POD_SELECTOR", default_value = "",
          help_heading = "Kubernetes")]
    pub pod_selector: String,

    /// Seconds between stream-reconnect checks.
    ///
    /// Pod discovery is event-driven (no polling delay). This interval only
    /// governs how quickly streams are restarted after the K8s API server
    /// closes them (typically every ~5 minutes).
    #[arg(long, env = "WATCH_INTERVAL", default_value = "10",
          help_heading = "Kubernetes")]
    pub watch_interval: u64,

    /// Container name to stream (default: first container in pod).
    #[arg(long, value_name = "NAME", help_heading = "Kubernetes")]
    pub container: Option<String>,

    /// Emit the last N log lines on (re)connect before going live (0 = disabled).
    ///
    /// Applied per pod at stream start.
    #[arg(long, env = "TAIL_LINES", default_value = "0", value_name = "N",
          help_heading = "Kubernetes")]
    pub tail_lines: i64,

    /// Start N seconds back on (re)connect.
    ///
    /// Covers the reconnect gap when the K8s API server closes the stream.
    /// Set to ≥ --watch-interval to avoid missing logs during reconnect.
    /// Ignored when --tail-lines is set.
    #[arg(long, env = "SINCE_SECONDS", default_value = "10", value_name = "SECS",
          help_heading = "Kubernetes")]
    pub since_seconds: i64,

    /// Exclude pods whose name matches this regex (repeatable, OR-combined).
    #[arg(long, value_name = "REGEX", help_heading = "Kubernetes")]
    pub exclude_pod: Vec<String>,

    /// Maximum number of concurrent pod log streams (0 = unlimited).
    ///
    /// Useful when watching namespaces with many pods to avoid overloading
    /// the Kubernetes API server.
    #[arg(long, env = "MAX_LOG_REQUESTS", default_value = "0", value_name = "N",
          help_heading = "Kubernetes")]
    pub max_log_requests: usize,

    // ---- Filter -----------------------------------------------------------

    /// Only forward lines matching this regex (repeatable, OR-combined).
    ///
    /// Kubernetes: matched against the log message (after the timestamp prefix).
    /// URL / socket: matched against the full raw line.
    #[arg(long, value_name = "REGEX", help_heading = "Filter")]
    pub include: Vec<String>,

    /// Drop lines matching this regex (repeatable, OR-combined, applied after --include).
    ///
    /// Kubernetes: matched against the log message (after the timestamp prefix).
    /// URL / socket: matched against the full raw line.
    #[arg(long, value_name = "REGEX", help_heading = "Filter")]
    pub exclude: Vec<String>,

    // ---- TCP Sink ---------------------------------------------------------

    /// Sink host. Output is ndjson only (one JSON object per line).
    #[arg(long, env = "TCP_SINK_HOST", default_value = "127.0.0.1",
          help_heading = "TCP Sink")]
    pub tcp_sink_host: String,

    /// Sink port. Compatible with Filebeat / Fluent Bit / Logstash tcp input.
    #[arg(long, env = "TCP_SINK_PORT", default_value = "5170",
          help_heading = "TCP Sink")]
    pub tcp_sink_port: u16,

    // ---- TCP Sink Connection ----------------------------------------------

    /// TCP connect timeout in ms (0 = no timeout).
    #[arg(long, env = "DEST_CONNECT_TIMEOUT", default_value = "10000",
          value_name = "MS", help_heading = "TCP Sink Connection")]
    pub dest_connect_timeout: u64,

    /// Per-write timeout in ms; reconnects if a write stalls (0 = disabled).
    #[arg(long, env = "DEST_READ_TIMEOUT", default_value = "0",
          value_name = "MS", help_heading = "TCP Sink Connection")]
    pub dest_read_timeout: u64,

    /// Idle timeout in ms; reconnects if no data is written for this long (0 = disabled).
    #[arg(long, env = "DEST_IDLE_TIMEOUT", default_value = "0",
          value_name = "MS", help_heading = "TCP Sink Connection")]
    pub dest_idle_timeout: u64,

    /// TCP keepalive interval in seconds (0 = disabled).
    #[arg(long, env = "DEST_KEEPALIVE_INTERVAL", default_value = "0",
          value_name = "SECS", help_heading = "TCP Sink Connection")]
    pub dest_keepalive_interval: u64,

    /// TCP keepalive probe timeout in seconds (0 = OS default).
    ///
    /// Only applied when --dest-keepalive-interval > 0.
    #[arg(long, env = "DEST_KEEPALIVE_TIMEOUT", default_value = "0",
          value_name = "SECS", help_heading = "TCP Sink Connection")]
    pub dest_keepalive_timeout: u64,

    // ---- TCP Sink Reconnect -----------------------------------------------

    /// Initial reconnect delay in ms; doubles each retry, capped at --dest-reconnect-max.
    #[arg(long, env = "DEST_RECONNECT_DELAY", default_value = "500",
          value_name = "MS", help_heading = "TCP Sink Reconnect")]
    pub dest_reconnect_delay: u64,

    /// Reconnect backoff ceiling in ms.
    #[arg(long, env = "DEST_RECONNECT_MAX", default_value = "30000",
          value_name = "MS", help_heading = "TCP Sink Reconnect")]
    pub dest_reconnect_max: u64,

    // ---- Health Check -----------------------------------------------------

    /// Address serving GET /healthz → 200 ok / 503 stale.
    #[arg(long, env = "HEALTHCHECK", default_value = "127.0.0.1:9091",
          value_name = "HOST:PORT",
          help_heading = "Health Check")]
    pub healthcheck: String,

    /// Seconds without a tick before /healthz returns 503 (≥ 3× --watch-interval).
    ///
    /// Kubernetes: tick per watch cycle.
    /// HTTP modes: background tick every 30 s.
    #[arg(long, env = "HEALTHCHECK_MAX_AGE", default_value = "30",
          value_name = "SECS",
          help_heading = "Health Check")]
    pub healthcheck_max_age: u64,

    // ---- Logging ----------------------------------------------------------

    /// Log format: "text" or "json".
    #[arg(long, env = "VIGIL_LOG_FORMAT", default_value = "text", help_heading = "Logging")]
    pub log_format: String,

    /// Enable debug logging with timestamps.
    #[arg(long, help_heading = "Logging")]
    pub debug: bool,
}
