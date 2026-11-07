//! When vigil-log-relay connects to its source again after a failure.

use clap::Args;

use crate::ReconnectConfig;

#[derive(Args, Debug)]
pub struct Reconnect {
    // ---- Source Reconnect -------------------------------------------------
    /// Initial delay in ms; doubles each retry, capped at --source-reconnect-max.
    ///
    /// Triggers: connection refused, timeout, HTTP non-2xx, read error.
    /// Clean stream EOF resets the counter and delay.
    #[arg(
        long,
        env = "SOURCE_RECONNECT_DELAY",
        default_value = "500",
        value_name = "MS",
        help_heading = "Source Reconnect"
    )]
    pub source_reconnect_delay: u64,

    /// Backoff ceiling in ms.
    #[arg(
        long,
        env = "SOURCE_RECONNECT_MAX",
        default_value = "30000",
        value_name = "MS",
        help_heading = "Source Reconnect"
    )]
    pub source_reconnect_max: u64,

    /// Max consecutive failures before exit (0 = unlimited).
    ///
    /// vigild then restarts the process via on-failure: restart.
    /// Clean EOF does not count.
    #[arg(
        long,
        env = "SOURCE_RECONNECT_RETRIES",
        default_value = "0",
        value_name = "N",
        help_heading = "Source Reconnect"
    )]
    pub source_reconnect_retries: u64,
}

impl Reconnect {
    pub fn reconnect_config(&self) -> ReconnectConfig {
        ReconnectConfig {
            initial_delay_ms: self.source_reconnect_delay,
            max_delay_ms: self.source_reconnect_max,
            max_retries: self.source_reconnect_retries,
        }
    }
}
