// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `services:` entries of a layer.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::{Override, append_unique};
use crate::signal::StopSignal;

mod multiline;
mod options;
mod watch_files;

pub use multiline::{LogsMultiline, MultilinePreset};
pub use options::{LogsForward, LogsPushFormat, OnExit, ReadyMode, ServiceType, Startup};
pub use watch_files::{FileChangeAction, WatchFiles};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ServiceConfig {
    /// `override: merge|replace` in the layer file.
    #[serde(default, rename = "override")]
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
    /// `B before: [A]` is equivalent to `A after: [B]` — B must be ready before A starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    /// Hard dependency: implies `after` ordering AND stops this service when
    /// a listed service stops for good (inactive or error; not while it is
    /// in backoff or stopping).
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
    /// How long a run of a `oneshot` may take before it is stopped and
    /// counts as failed. Default: no limit; `0s` is no limit too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_timeout: Option<String>,
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
    /// May be combined with `logs_push_addr`; entries then go to both.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_push_socket: Option<String>,
    /// TCP address (`host:port`) to connect to and push log entries (ndjson by default).
    /// May be combined with `logs_push_socket`; entries then go to both.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_push_addr: Option<String>,
    /// Format for pushed log lines. Default: `ndjson`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_push_format: Option<LogsPushFormat>,
    /// Join the lines of a stack trace into one log entry. A later layer
    /// replaces the whole block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_multiline: Option<LogsMultiline>,
}

impl ServiceConfig {
    /// Effective `startup` (default `disabled`).
    pub fn startup(&self) -> Startup {
        self.startup.unwrap_or_default()
    }
}

pub(super) fn merge_service(base: &mut ServiceConfig, overlay: &ServiceConfig) {
    merge_opt!(
        base,
        overlay,
        [
            summary,
            description,
            command,
            service_type,
            schedule,
            schedule_tz,
            startup,
            pass_env,
            user,
            user_id,
            group,
            group_id,
            working_dir,
            stop_signal,
            kill_delay,
            ready,
            ready_check,
            ready_timeout,
            healthy_time,
            run_timeout,
            on_success,
            on_failure,
            backoff_delay,
            backoff_factor,
            backoff_limit,
            logs_forward,
            logs_push_socket,
            logs_push_addr,
            logs_push_format,
            logs_multiline,
        ]
    );
    append_unique(&mut base.after, &overlay.after);
    append_unique(&mut base.before, &overlay.before);
    append_unique(&mut base.requires, &overlay.requires);
    append_unique(&mut base.env_file, &overlay.env_file);
    append_unique(&mut base.pass_env_glob, &overlay.pass_env_glob);
    append_unique(&mut base.pass_env_regex, &overlay.pass_env_regex);
    base.environment.extend(overlay.environment.clone());
    base.exit_code_map.extend(overlay.exit_code_map.clone());
    base.on_check_failure
        .extend(overlay.on_check_failure.clone());
    if !overlay.success_exit_codes.is_empty() {
        base.success_exit_codes = overlay.success_exit_codes.clone();
    }
    if let Some(o) = &overlay.watch_files {
        let w = base.watch_files.get_or_insert_with(WatchFiles::default);
        merge_opt!(w, o, [on_change, interval, paths]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
