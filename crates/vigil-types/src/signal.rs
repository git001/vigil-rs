// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use nix::sys::signal::Signal;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// A POSIX signal used as a service stop signal.
/// Serialises/deserialises from strings like "SIGTERM", "SIGUSR1", etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StopSignal(pub Signal);

impl Default for StopSignal {
    fn default() -> Self {
        StopSignal(Signal::SIGTERM)
    }
}

impl fmt::Display for StopSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<StopSignal> for String {
    fn from(s: StopSignal) -> String {
        s.0.to_string()
    }
}

#[derive(Debug, Error)]
#[error("unknown signal: {0}")]
pub struct UnknownSignal(String);

impl TryFrom<String> for StopSignal {
    type Error = UnknownSignal;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Signal::from_str(&s)
            .map(StopSignal)
            .map_err(|_| UnknownSignal(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_sigterm() {
        assert_eq!(StopSignal::default().0, Signal::SIGTERM);
    }

    #[test]
    fn display_shows_signal_name() {
        assert_eq!(StopSignal(Signal::SIGTERM).to_string(), "SIGTERM");
        assert_eq!(StopSignal(Signal::SIGUSR1).to_string(), "SIGUSR1");
        assert_eq!(StopSignal(Signal::SIGKILL).to_string(), "SIGKILL");
    }

    #[test]
    fn try_from_valid_string() {
        let s = StopSignal::try_from("SIGTERM".to_string()).unwrap();
        assert_eq!(s.0, Signal::SIGTERM);
        let s2 = StopSignal::try_from("SIGUSR2".to_string()).unwrap();
        assert_eq!(s2.0, Signal::SIGUSR2);
    }

    #[test]
    fn try_from_invalid_string() {
        let err = StopSignal::try_from("NOSUCHSIGNAL".to_string()).unwrap_err();
        assert!(err.to_string().contains("unknown signal"));
        assert!(err.to_string().contains("NOSUCHSIGNAL"));
    }

    #[test]
    fn into_string_roundtrip() {
        let original = StopSignal(Signal::SIGUSR2);
        let s: String = original.into();
        assert_eq!(s, "SIGUSR2");
        let back = StopSignal::try_from(s).unwrap();
        assert_eq!(back.0, Signal::SIGUSR2);
    }
}
