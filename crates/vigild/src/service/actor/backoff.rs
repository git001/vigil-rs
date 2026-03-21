// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Backoff helpers — delay calculation and limit checking.

use std::time::Duration;

use crate::duration::parse_duration;

use super::super::{DEFAULT_BACKOFF_DELAY, DEFAULT_BACKOFF_FACTOR, DEFAULT_BACKOFF_LIMIT};
use super::Actor;

impl Actor {
    // -----------------------------------------------------------------------
    // Backoff helpers
    // -----------------------------------------------------------------------

    pub(super) fn next_backoff(&mut self) -> Duration {
        let delay = self.current_backoff;

        let factor = self.config.backoff_factor.unwrap_or(DEFAULT_BACKOFF_FACTOR);
        let limit = self
            .config
            .backoff_limit
            .as_deref()
            .and_then(|s| parse_duration(s).ok())
            .unwrap_or(DEFAULT_BACKOFF_LIMIT);
        let next =
            Duration::from_millis(((self.current_backoff.as_millis() as f64) * factor) as u64)
                .min(limit);

        self.current_backoff = next;
        self.backoff_count += 1;
        delay
    }

    pub(super) fn reset_backoff(&mut self) {
        self.backoff_count = 0;
        self.current_backoff = self
            .config
            .backoff_delay
            .as_deref()
            .and_then(|s| parse_duration(s).ok())
            .unwrap_or(DEFAULT_BACKOFF_DELAY);
    }

    pub(super) fn backoff_limit_exceeded(&self) -> bool {
        let limit = self
            .config
            .backoff_limit
            .as_deref()
            .and_then(|s| parse_duration(s).ok())
            .unwrap_or(DEFAULT_BACKOFF_LIMIT);
        self.backoff_count > 0 && self.current_backoff >= limit
    }
}
