// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! How many worker threads the tokio runtime of vigild and vigil-log-relay
//! gets.
//!
//! tokio's default is one worker per CPU the process can see. Without a CPU
//! limit that is every core of the node, and each worker costs about 2 MiB of
//! memory, although a supervisor's work (processes, checks, the API) is I/O
//! and needs few. Blocking work runs in tokio's separate blocking pool.

use std::num::NonZeroUsize;

/// Workers when none are requested (fewer when fewer CPUs are available).
pub const DEFAULT_WORKER_THREADS: usize = 2;

/// Upper bound of tokio's blocking pool (file reads, user lookups), instead
/// of tokio's 512. Reads have a time limit; when this many hang (e.g. on a
/// dead NFS mount), further ones wait in the queue until theirs runs out
/// instead of each taking another thread.
pub const MAX_BLOCKING_THREADS: usize = 16;

/// `requested`, or [`DEFAULT_WORKER_THREADS`] but not more than `available`
/// CPUs (at least 1).
pub fn worker_threads(requested: Option<NonZeroUsize>, available: usize) -> usize {
    match requested {
        Some(n) => n.get(),
        None => available.clamp(1, DEFAULT_WORKER_THREADS),
    }
}

/// The CPUs this process may use (affinity and cgroup quota), at least 1.
pub fn available_cpus() -> usize {
    std::thread::available_parallelism().map_or(1, NonZeroUsize::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_capped_by_the_cpus() {
        assert_eq!(worker_threads(None, 64), DEFAULT_WORKER_THREADS);
        assert_eq!(worker_threads(None, 2), 2);
        assert_eq!(worker_threads(None, 1), 1);
        assert_eq!(worker_threads(None, 0), 1);
    }

    #[test]
    fn a_requested_number_wins() {
        let eight = NonZeroUsize::new(8);
        assert_eq!(worker_threads(eight, 2), 8);
        assert_eq!(worker_threads(NonZeroUsize::new(1), 64), 1);
    }
}
