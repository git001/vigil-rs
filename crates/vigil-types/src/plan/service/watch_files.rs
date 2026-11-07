// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `watch-files`: react when files a service depends on change.

use serde::{Deserialize, Serialize};

use crate::signal::StopSignal;

/// What to do when a watched file changes (`watch-files.on-change`).
///
/// Written as `restart`, `ignore`, `signal` (= `SIGHUP`) or
/// `signal:<NAME>` (e.g. `signal:SIGUSR1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum FileChangeAction {
    #[default]
    Restart,
    Ignore,
    Signal(StopSignal),
}

impl TryFrom<String> for FileChangeAction {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        match s.as_str() {
            "restart" => Ok(Self::Restart),
            "ignore" => Ok(Self::Ignore),
            "signal" => Ok(Self::Signal(StopSignal(nix::sys::signal::Signal::SIGHUP))),
            other => match other.strip_prefix("signal:") {
                Some(name) => StopSignal::try_from(name.to_string())
                    .map(Self::Signal)
                    .map_err(|e| format!("on-change {s:?}: {e}")),
                None => Err(format!(
                    "on-change {s:?}: expected restart, ignore, signal or signal:<NAME>"
                )),
            },
        }
    }
}

impl From<FileChangeAction> for String {
    fn from(a: FileChangeAction) -> String {
        match a {
            FileChangeAction::Restart => "restart".into(),
            FileChangeAction::Ignore => "ignore".into(),
            FileChangeAction::Signal(sig) => format!("signal:{sig}"),
        }
    }
}

/// `watch-files`: react when files the service depends on change.
///
/// Without this block the defaults apply: the files read by `{{ file() }}`
/// and `env-file` at the last start are watched every 30s, a change
/// restarts the service.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct WatchFiles {
    /// Default `restart`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_change: Option<FileChangeAction>,
    /// Polling interval. Default `30s`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    /// Watch only these paths instead of the files read at start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::signal::Signal;

    #[test]
    fn on_change_forms() {
        let p = |s: &str| FileChangeAction::try_from(s.to_string());
        assert_eq!(p("restart"), Ok(FileChangeAction::Restart));
        assert_eq!(p("ignore"), Ok(FileChangeAction::Ignore));
        assert_eq!(
            p("signal"),
            Ok(FileChangeAction::Signal(StopSignal(Signal::SIGHUP)))
        );
        assert_eq!(
            p("signal:SIGUSR1"),
            Ok(FileChangeAction::Signal(StopSignal(Signal::SIGUSR1)))
        );
        assert!(p("signal:NOPE").unwrap_err().contains("unknown signal"));
        assert!(p("reload").unwrap_err().contains("expected restart"));
        assert_eq!(
            String::from(FileChangeAction::Signal(StopSignal(Signal::SIGHUP))),
            "signal:SIGHUP"
        );
    }
}
