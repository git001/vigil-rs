//! TCP keepalive of the source and the TCP sink connections, set the same
//! way on both: `--*-keepalive-interval` between probes, the first probe
//! after `--*-keepalive-timeout` of silence (3 x the interval when 0), and
//! 3 unanswered probes close the connection.

use std::time::Duration;

/// Unanswered probes before the kernel closes the connection.
pub const RETRIES: u32 = 3;

#[derive(Debug, Clone, Copy, Default)]
pub struct Keepalive {
    /// Seconds between probes (0 = keepalive off).
    pub interval_secs: u64,
    /// Seconds of silence before the first probe (0 = 3 x the interval).
    pub timeout_secs: u64,
}

impl Keepalive {
    /// Silence before the first probe and the interval between probes;
    /// `None` when keepalive is off.
    pub fn times(&self) -> Option<(Duration, Duration)> {
        if self.interval_secs == 0 {
            return None;
        }
        let idle = if self.timeout_secs > 0 {
            self.timeout_secs
        } else {
            self.interval_secs * 3
        };
        Some((
            Duration::from_secs(idle),
            Duration::from_secs(self.interval_secs),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_without_interval() {
        let k = Keepalive {
            interval_secs: 0,
            timeout_secs: 30,
        };
        assert_eq!(k.times(), None);
    }

    #[test]
    fn idle_defaults_to_three_intervals() {
        let k = Keepalive {
            interval_secs: 10,
            timeout_secs: 0,
        };
        assert_eq!(
            k.times(),
            Some((Duration::from_secs(30), Duration::from_secs(10)))
        );
        let k = Keepalive {
            interval_secs: 10,
            timeout_secs: 60,
        };
        assert_eq!(
            k.times(),
            Some((Duration::from_secs(60), Duration::from_secs(10)))
        );
    }
}
