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
        Signal::from_str(&s).map(StopSignal).map_err(|_| UnknownSignal(s))
    }
}
