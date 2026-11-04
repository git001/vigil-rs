// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::Override;
use crate::signal::StopSignal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Startup {
    #[default]
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogsForward {
    /// Forward service stdout/stderr to vigild's stdout/stderr (default).
    /// Allows `podman logs` / `docker logs` to capture service output.
    #[default]
    Enabled,
    /// Keep service logs internal (accessible via `vigil logs` / the API).
    /// The service's stdout/stderr is captured and stored in the log buffer
    /// but NOT printed to vigild's own stdout/stderr.
    Disabled,
    /// Let the service's stdout/stderr pass directly to the container's
    /// stdout/stderr without any capture or buffering by vigild.
    /// Use this for log-collector services (Vector, Filebeat, …) that
    /// format and emit their own output — vigild must not interfere.
    Passthrough,
}

/// Format used when pushing log entries to an external socket/address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogsPushFormat {
    /// Newline-delimited JSON — one `LogEntry` JSON object per line.
    #[default]
    Ndjson,
}

/// How a service's process is meant to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceType {
    /// A long-running process (default).
    #[default]
    Simple,
    /// A job that runs to completion. Exit success ends in state `done`;
    /// services ordered after it start then. Optionally run on a
    /// `schedule`.
    Oneshot,
}

/// When a started service counts as ready (state `active`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadyMode {
    /// Ready as soon as the process is spawned (v3 behaviour).
    #[default]
    Process,
    /// Ready when the service sends `READY=1` to `$NOTIFY_SOCKET`
    /// (sd_notify protocol).
    Notify,
    /// Ready when the check named in `ready-check` passes for the first time
    /// after the process was spawned.
    Check,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnExit {
    Restart,
    /// Shut down the daemon. Exit code: the service's code with
    /// `--exit-code propagate` (default), else success→0, failure→10.
    Shutdown,
    /// Shut down the daemon with a failure code: the service's non-zero code
    /// with `--exit-code propagate`, else (or for code 0) 10.
    FailureShutdown,
    /// Shut down the daemon with exit code 0.
    SuccessShutdown,
    Ignore,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ServiceConfig {
    #[serde(default)]
    pub override_mode: Override,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// `simple` (default) or `oneshot`.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub service_type: Option<ServiceType>,
    /// Cron pattern (5 fields, or 6 with seconds first) for a `oneshot`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    /// IANA time zone for `schedule` (default `UTC`), e.g. `Europe/Vienna`.
    /// Needs the system time zone database (`tzdata`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_tz: Option<String>,
    /// `None`: not set in this layer (a merge keeps the earlier value);
    /// read it with [`ServiceConfig::startup`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup: Option<Startup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    /// Syntactic sugar for `after` from the reverse direction.
    /// `B before: [A]` is equivalent to `A after: [B]` — B must be running before A starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    /// Hard dependency: implies `after` ordering AND stops this service if any
    /// listed service leaves the running state (Inactive, Backoff, Error).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub environment: IndexMap<String, String>,
    /// Files with `KEY=value` lines (e.g. written by the Vault Agent), read
    /// on every start. Their variables come after pass-env and before
    /// `environment`. A leading `-` makes a file optional.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_file: Vec<String>,
    /// React to changes of files the service depends on; see [`WatchFiles`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watch_files: Option<WatchFiles>,
    /// Pass vigild's whole environment to the service. Default (unset):
    /// `false`, i.e. only PATH, HOME, USER, LOGNAME, TZ plus variables
    /// matched by `pass-env-glob` / `pass-env-regex`, plus `environment`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pass_env: Option<bool>,
    /// Names of vigild environment variables to pass through; `*` and `?`
    /// wildcards, the pattern must match the whole name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pass_env_glob: Vec<String>,
    /// Regular expressions (anchored to the whole name) selecting vigild
    /// environment variables to pass through.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pass_env_regex: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// Custom stop signal (default: SIGTERM).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_signal: Option<StopSignal>,
    /// Grace period before SIGKILL after stop_signal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kill_delay: Option<String>,
    /// When the service counts as ready. Default: `process`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready: Option<ReadyMode>,
    /// Check that signals readiness (required for `ready: check`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_check: Option<String>,
    /// How long to wait for readiness before the start counts as failed and
    /// the service is stopped. Default `30s`; `0s` waits forever.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_timeout: Option<String>,
    /// How long the service must stay `active` before it counts as healthy
    /// again: a service alert resolves and the checks of the service may
    /// alert again. Default `10s`, allowed `1s` to `1h`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub healthy_time: Option<String>,
    /// Exit codes that count as success (after `exit-code-map`).
    /// Default (empty): `[0]`. A signal counts as code `128 + signal`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub success_exit_codes: Vec<i32>,
    /// Rewrite exit codes before they are classified and propagated,
    /// e.g. `{143: 0}` to treat SIGTERM as a clean exit.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub exit_code_map: IndexMap<i32, i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_success: Option<OnExit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<OnExit>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub on_check_failure: IndexMap<String, OnExit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backoff_delay: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backoff_factor: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backoff_limit: Option<String>,
    /// Controls how vigild handles stdout/stderr. Default: `enabled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logs_forward: Option<LogsForward>,
    /// Unix socket path to connect to and push log entries (ndjson by default).
    /// Mutually exclusive with `logs_push_addr`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_push_socket: Option<String>,
    /// TCP address (`host:port`) to connect to and push log entries (ndjson by default).
    /// Mutually exclusive with `logs_push_socket`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_push_addr: Option<String>,
    /// Format for pushed log lines. Default: `ndjson`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_push_format: Option<LogsPushFormat>,
}

impl ServiceConfig {
    /// Effective `startup` (default `disabled`).
    pub fn startup(&self) -> Startup {
        self.startup.unwrap_or_default()
    }
}

pub(super) fn merge_service(base: &mut ServiceConfig, overlay: &ServiceConfig) {
    macro_rules! opt {
        ($f:ident) => {
            if overlay.$f.is_some() {
                base.$f = overlay.$f.clone();
            }
        };
    }

    opt!(summary);
    opt!(description);
    opt!(command);
    opt!(service_type);
    opt!(schedule);
    opt!(schedule_tz);
    opt!(startup);
    for v in &overlay.after {
        if !base.after.contains(v) {
            base.after.push(v.clone());
        }
    }
    for v in &overlay.before {
        if !base.before.contains(v) {
            base.before.push(v.clone());
        }
    }
    for v in &overlay.requires {
        if !base.requires.contains(v) {
            base.requires.push(v.clone());
        }
    }
    base.environment.extend(
        overlay
            .environment
            .iter()
            .map(|(k, v)| (k.clone(), v.clone())),
    );
    for v in &overlay.env_file {
        if !base.env_file.contains(v) {
            base.env_file.push(v.clone());
        }
    }
    opt!(pass_env);
    for v in &overlay.pass_env_glob {
        if !base.pass_env_glob.contains(v) {
            base.pass_env_glob.push(v.clone());
        }
    }
    for v in &overlay.pass_env_regex {
        if !base.pass_env_regex.contains(v) {
            base.pass_env_regex.push(v.clone());
        }
    }
    opt!(user);
    opt!(user_id);
    opt!(group);
    opt!(group_id);
    opt!(working_dir);
    opt!(stop_signal);
    opt!(kill_delay);
    opt!(ready);
    opt!(ready_check);
    opt!(ready_timeout);
    opt!(healthy_time);
    if let Some(o) = &overlay.watch_files {
        let w = base.watch_files.get_or_insert_with(WatchFiles::default);
        if o.on_change.is_some() {
            w.on_change = o.on_change;
        }
        if o.interval.is_some() {
            w.interval = o.interval.clone();
        }
        if o.paths.is_some() {
            w.paths = o.paths.clone();
        }
    }
    if !overlay.success_exit_codes.is_empty() {
        base.success_exit_codes = overlay.success_exit_codes.clone();
    }
    base.exit_code_map
        .extend(overlay.exit_code_map.iter().map(|(k, v)| (*k, *v)));
    opt!(on_success);
    opt!(on_failure);
    base.on_check_failure.extend(
        overlay
            .on_check_failure
            .iter()
            .map(|(k, v)| (k.clone(), *v)),
    );
    opt!(backoff_delay);
    opt!(backoff_factor);
    opt!(backoff_limit);
    opt!(logs_forward);
    opt!(logs_push_socket);
    opt!(logs_push_addr);
    opt!(logs_push_format);
}

#[cfg(test)]
mod watch_files_tests {
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

    #[test]
    fn watch_files_merge_per_field() {
        let mut base = ServiceConfig {
            watch_files: Some(WatchFiles {
                on_change: Some(FileChangeAction::Ignore),
                interval: Some("10s".into()),
                paths: None,
            }),
            ..Default::default()
        };
        let overlay = ServiceConfig {
            watch_files: Some(WatchFiles {
                paths: Some(vec!["/etc/app.conf".into()]),
                ..Default::default()
            }),
            ..Default::default()
        };
        merge_service(&mut base, &overlay);
        let w = base.watch_files.unwrap();
        assert_eq!(w.on_change, Some(FileChangeAction::Ignore));
        assert_eq!(w.interval.as_deref(), Some("10s"));
        assert_eq!(w.paths, Some(vec!["/etc/app.conf".to_string()]));
    }
}
