//! Where vigil-log-relay sends the log lines, and which ones.

use clap::Args;

use crate::SinkConfig;
use crate::keepalive::Keepalive;

#[derive(Args, Debug)]
pub struct Sink {
    // ---- Filter -----------------------------------------------------------
    /// Only forward lines matching this regex (repeatable, OR-combined).
    ///
    /// Kubernetes: matched against the log message (after the timestamp prefix).
    /// URL / socket: matched against the full raw line.
    /// The environment variable holds one regex; combine several with `|`.
    #[arg(long, env = "INCLUDE", value_name = "REGEX", help_heading = "Filter")]
    pub include: Vec<String>,

    /// Drop lines matching this regex (repeatable, OR-combined, applied after --include).
    ///
    /// Kubernetes: matched against the log message (after the timestamp prefix).
    /// URL / socket: matched against the full raw line.
    /// The environment variable holds one regex; combine several with `|`.
    #[arg(long, env = "EXCLUDE", value_name = "REGEX", help_heading = "Filter")]
    pub exclude: Vec<String>,

    // ---- Sink -------------------------------------------------------------
    /// Write forwarded lines to stdout instead of a TCP socket.
    ///
    /// When set, all TCP sink options are ignored and no TCP connection is made.
    /// Useful for debugging or piping output to another process.
    #[arg(long, env = "STDOUT_SINK", help_heading = "Sink")]
    pub stdout_sink: bool,

    // ---- TCP Sink ---------------------------------------------------------
    /// Sink host. Output is ndjson only (one JSON object per line).
    #[arg(
        long,
        env = "TCP_SINK_HOST",
        default_value = "127.0.0.1",
        help_heading = "TCP Sink"
    )]
    pub tcp_sink_host: String,

    /// Sink port. Compatible with Filebeat / Fluent Bit / Logstash tcp input.
    #[arg(
        long,
        env = "TCP_SINK_PORT",
        default_value = "5170",
        help_heading = "TCP Sink"
    )]
    pub tcp_sink_port: u16,

    // ---- TCP Sink Connection ----------------------------------------------
    /// TCP connect timeout in ms (0 = no timeout).
    #[arg(
        long,
        env = "DEST_CONNECT_TIMEOUT",
        default_value = "10000",
        value_name = "MS",
        help_heading = "TCP Sink Connection"
    )]
    pub dest_connect_timeout: u64,

    /// Per-write timeout in ms; reconnects if a write stalls (0 = disabled).
    #[arg(
        long,
        env = "DEST_WRITE_TIMEOUT",
        default_value = "0",
        value_name = "MS",
        help_heading = "TCP Sink Connection"
    )]
    pub dest_write_timeout: u64,

    /// Idle timeout in ms; reconnects if no data is written for this long (0 = disabled).
    #[arg(
        long,
        env = "DEST_IDLE_TIMEOUT",
        default_value = "0",
        value_name = "MS",
        help_heading = "TCP Sink Connection"
    )]
    pub dest_idle_timeout: u64,

    /// TCP keepalive: seconds between probes (0 = off); 3 unanswered probes
    /// close the connection.
    #[arg(
        long,
        env = "DEST_KEEPALIVE_INTERVAL",
        default_value = "0",
        value_name = "SECS",
        help_heading = "TCP Sink Connection"
    )]
    pub dest_keepalive_interval: u64,

    /// TCP keepalive: seconds of silence before the first probe
    /// (0 = 3 x --dest-keepalive-interval).
    ///
    /// Only applied when --dest-keepalive-interval > 0.
    #[arg(
        long,
        env = "DEST_KEEPALIVE_TIMEOUT",
        default_value = "0",
        value_name = "SECS",
        help_heading = "TCP Sink Connection"
    )]
    pub dest_keepalive_timeout: u64,

    // ---- TCP Sink Reconnect -----------------------------------------------
    /// Initial reconnect delay in ms; doubles each retry, capped at --dest-reconnect-max.
    #[arg(
        long,
        env = "DEST_RECONNECT_DELAY",
        default_value = "500",
        value_name = "MS",
        help_heading = "TCP Sink Reconnect"
    )]
    pub dest_reconnect_delay: u64,

    /// Reconnect backoff ceiling in ms.
    #[arg(
        long,
        env = "DEST_RECONNECT_MAX",
        default_value = "30000",
        value_name = "MS",
        help_heading = "TCP Sink Reconnect"
    )]
    pub dest_reconnect_max: u64,
}

impl Sink {
    /// The connection settings of the TCP sink.
    pub fn tcp_sink_config(&self) -> SinkConfig {
        SinkConfig {
            connect_timeout_ms: self.dest_connect_timeout,
            write_timeout_ms: self.dest_write_timeout,
            idle_timeout_ms: self.dest_idle_timeout,
            keepalive: Keepalive {
                interval_secs: self.dest_keepalive_interval,
                timeout_secs: self.dest_keepalive_timeout,
            },
            reconnect_delay_ms: self.dest_reconnect_delay,
            reconnect_max_ms: self.dest_reconnect_max,
        }
    }
}
