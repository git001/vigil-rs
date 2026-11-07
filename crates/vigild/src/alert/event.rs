// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The alert event every component (checks, services, vigild) sends.

use vigil_types::api::CheckStatus;

/// Component an alert event comes from; selects the subscription list
/// (`on-check`, `on-service`, `on-vigild`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertSource {
    Check,
    Service,
    Vigild,
}

impl AlertSource {
    pub fn as_str(self) -> &'static str {
        match self {
            AlertSource::Check => "check",
            AlertSource::Service => "service",
            AlertSource::Vigild => "vigild",
        }
    }
}

/// Event name of every resolved event.
pub const RECOVERED: &str = "recovered";

/// One alert: a problem (`firing`) or its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertEvent {
    pub source: AlertSource,
    /// Name of the check or service; for vigild the subject (`layers`).
    pub name: String,
    /// What happened, e.g. `failed`, `backoff`, `invalid`; `recovered` for
    /// a resolved event.
    pub event: String,
    pub firing: bool,
    /// Human-readable detail, e.g. `exited with code 3, restarting in 2s`.
    pub message: String,
}

impl AlertEvent {
    /// A problem reported by `source`/`name`.
    pub fn firing(
        source: AlertSource,
        name: impl Into<String>,
        event: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            source,
            name: name.into(),
            event: event.into(),
            firing: true,
            message: message.into(),
        }
    }

    /// The end of the problem `source`/`name` reported before.
    pub fn resolved(
        source: AlertSource,
        name: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            source,
            name: name.into(),
            event: RECOVERED.into(),
            firing: false,
            message: message.into(),
        }
    }

    /// A check status change: `failed` for down, `recovered` for up.
    pub fn check(name: &str, status: CheckStatus, error: Option<&str>) -> Self {
        if status == CheckStatus::Down {
            let message = match error {
                Some(e) => format!("check {name} failed: {e}"),
                None => format!("check {name} failed"),
            };
            Self::firing(AlertSource::Check, name, "failed", message)
        } else {
            Self::resolved(AlertSource::Check, name, format!("check {name} recovered"))
        }
    }

    /// `down` for a problem, `up` for its end (the `status` field of the
    /// payloads).
    pub fn status_str(&self) -> &'static str {
        if self.firing { "down" } else { "up" }
    }
}
