//! Options of the Kubernetes source (`--kubernetes`).

use clap::Args;

#[derive(Args, Debug)]
pub struct Kubernetes {
    // ---- Kubernetes -------------------------------------------------------
    /// Namespace to watch.
    #[arg(
        long,
        env = "NAMESPACE",
        default_value = "default",
        help_heading = "Kubernetes"
    )]
    pub namespace: String,

    /// Label selector, e.g. "app=myapp".
    #[arg(
        long,
        env = "POD_SELECTOR",
        default_value = "",
        help_heading = "Kubernetes"
    )]
    pub pod_selector: String,

    /// Seconds between stream-reconnect checks.
    ///
    /// Pod discovery is event-driven (no polling delay). This interval only
    /// governs how quickly streams are restarted after the K8s API server
    /// closes them (typically every ~5 minutes).
    #[arg(
        long,
        env = "WATCH_INTERVAL",
        default_value = "10",
        help_heading = "Kubernetes"
    )]
    pub watch_interval: u64,

    /// Container(s) to stream. Use "all" for every container in the pod,
    /// or a comma-separated list of names, e.g. "app,sidecar".
    /// Omit to stream the default container (K8s picks first / annotated).
    #[arg(
        long,
        env = "CONTAINER_SELECTOR",
        value_name = "NAME|all|n1,n2,...",
        help_heading = "Kubernetes"
    )]
    pub container: Option<String>,

    /// Emit the last N log lines on (re)connect before going live (0 = disabled).
    ///
    /// Applied per pod at stream start.
    #[arg(
        long,
        env = "TAIL_LINES",
        default_value = "0",
        value_name = "N",
        help_heading = "Kubernetes"
    )]
    pub tail_lines: i64,

    /// Start N seconds back on (re)connect.
    ///
    /// Covers the reconnect gap when the K8s API server closes the stream.
    /// Set to ≥ --watch-interval to avoid missing logs during reconnect.
    /// Ignored when --tail-lines is set.
    #[arg(
        long,
        env = "SINCE_SECONDS",
        default_value = "10",
        value_name = "SECS",
        help_heading = "Kubernetes"
    )]
    pub since_seconds: i64,

    /// Exclude pods whose name matches this regex (repeatable, OR-combined).
    #[arg(long, value_name = "REGEX", help_heading = "Kubernetes")]
    pub exclude_pod: Vec<String>,

    /// Maximum number of concurrent pod log streams (0 = unlimited).
    ///
    /// Useful when watching namespaces with many pods to avoid overloading
    /// the Kubernetes API server.
    #[arg(
        long,
        env = "MAX_LOG_REQUESTS",
        default_value = "0",
        value_name = "N",
        help_heading = "Kubernetes"
    )]
    pub max_log_requests: usize,

    /// Never use the K8s stream= query parameter; always read one combined
    /// stdout+stderr stream.
    ///
    /// Without this flag the relay tries separate streams on K8s >= 1.32 and
    /// falls back to the combined stream by itself if the API server rejects
    /// the parameter (feature gate PodLogsQuerySplitStreams off, or admission
    /// control). The flag only saves that first rejected request.
    #[arg(long, env = "NO_STREAM_PARAM", help_heading = "Kubernetes")]
    pub no_stream_param: bool,
}
