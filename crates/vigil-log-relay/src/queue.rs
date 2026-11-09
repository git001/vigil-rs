//! The channel between the sources and the sink.
//!
//! It holds `--send-buffer-lines` lines. A line that does not fit (the sink
//! is down or too slow) is dropped and counted; [`report_drops`] sums the
//! drops up in one warning per second instead of one per line, which would
//! flood the relay's own log during a long sink outage.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::warn;

/// How often dropped lines are reported.
const REPORT_EVERY: Duration = Duration::from_secs(1);

/// A count of dropped lines.
pub struct Drops(AtomicU64);

impl Drops {
    pub const fn new() -> Self {
        Drops(AtomicU64::new(0))
    }

    fn add(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// The lines dropped since the last call.
    fn take(&self) -> u64 {
        self.0.swap(0, Ordering::Relaxed)
    }
}

/// The drops of this process.
static DROPS: Drops = Drops::new();

/// Queue `line` for the sink, or drop and count it if the channel is full.
pub fn send(tx: &mpsc::Sender<String>, line: String) {
    send_counted(tx, line, &DROPS);
}

fn send_counted(tx: &mpsc::Sender<String>, line: String, drops: &Drops) {
    if tx.try_send(line).is_err() {
        drops.add();
    }
}

/// Warn once per second about the lines dropped in that second.
/// `capacity` is the channel size, for the message. Runs until the process ends.
pub async fn report_drops(capacity: usize) {
    let mut every = tokio::time::interval(REPORT_EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        report(&DROPS, capacity);
    }
}

/// Log the drops counted in `drops`, if any; returns how many.
fn report(drops: &Drops, capacity: usize) -> u64 {
    let dropped = drops.take();
    if dropped > 0 {
        warn!(
            dropped,
            buffer_lines = capacity,
            "send buffer full — dropped log lines (the sink is down or too slow; \
             see --send-buffer-lines)"
        );
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_that_do_not_fit_are_counted_and_reported_once() {
        let drops = Drops::new();
        let (tx, mut rx) = mpsc::channel(2);
        for i in 0..5 {
            send_counted(&tx, format!("line {i}\n"), &drops);
        }
        assert_eq!(rx.try_recv().unwrap(), "line 0\n");
        assert_eq!(rx.try_recv().unwrap(), "line 1\n");
        assert!(rx.try_recv().is_err());
        assert_eq!(report(&drops, 2), 3);
        // Counted anew after a report.
        assert_eq!(report(&drops, 2), 0);
    }

    #[test]
    fn a_closed_channel_counts_as_a_drop() {
        let drops = Drops::new();
        let (tx, rx) = mpsc::channel(2);
        drop(rx);
        send_counted(&tx, "x\n".to_string(), &drops);
        assert_eq!(drops.take(), 1);
    }
}
