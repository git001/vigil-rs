// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Validation of a service beyond deserialization.

use anyhow::Context;
use vigil_types::plan::{Plan, ServiceConfig};

use crate::process_util::{EnvPolicy, REDUNDANT_LISTS, redundant_lists};

use super::dependencies::{dependency_warnings, validate_dependency_names};
use crate::duration::parse_field;

pub(super) fn validate_service(plan: &Plan, name: &str, svc: &ServiceConfig) -> anyhow::Result<()> {
    // Checked on the merged plan: a layer may leave `command` to an earlier one.
    anyhow::ensure!(
        svc.command.as_deref().is_some_and(|c| !c.trim().is_empty()),
        "command is required"
    );
    EnvPolicy::for_service(svc)?;
    if redundant_lists(svc.pass_env, &svc.pass_env_glob, &svc.pass_env_regex) {
        tracing::warn!(service = %name, "{REDUNDANT_LISTS}");
    }
    validate_ready(plan, svc)?;
    validate_dependency_names(plan, name, svc)?;
    for warning in dependency_warnings(plan, name, svc) {
        tracing::warn!(service = %name, "{warning}");
    }
    validate_oneshot(svc)?;
    crate::template::check_service(svc)?;
    validate_watch_files(svc)?;
    validate_healthy_time(svc)?;
    parse_field("kill-delay", svc.kill_delay.as_deref())?;
    let delay = parse_field("backoff-delay", svc.backoff_delay.as_deref())?;
    let limit = parse_field("backoff-limit", svc.backoff_limit.as_deref())?;
    if let Some(d) = delay {
        // 0 would never grow, so the backoff limit is never reached and a
        // failing service restarts in a tight loop.
        anyhow::ensure!(!d.is_zero(), "backoff-delay must be greater than 0");
    }
    let (delay, limit) = (
        delay.unwrap_or(crate::service::DEFAULT_BACKOFF_DELAY),
        limit.unwrap_or(crate::service::DEFAULT_BACKOFF_LIMIT),
    );
    anyhow::ensure!(
        limit >= delay,
        "backoff-limit ({limit:?}) must not be shorter than backoff-delay ({delay:?})"
    );
    if let Some(f) = svc.backoff_factor {
        anyhow::ensure!(
            f.is_finite() && f >= 1.0,
            "backoff-factor {f} must be a number of at least 1.0"
        );
    }
    Ok(())
}

fn validate_healthy_time(svc: &ServiceConfig) -> anyhow::Result<()> {
    use crate::service::{MAX_HEALTHY_TIME, MIN_HEALTHY_TIME};
    if let Some(d) = parse_field("healthy-time", svc.healthy_time.as_deref())? {
        anyhow::ensure!(
            (MIN_HEALTHY_TIME..=MAX_HEALTHY_TIME).contains(&d),
            "healthy-time {:?} must be between 1s and 1h",
            svc.healthy_time.as_deref().unwrap_or_default()
        );
    }
    Ok(())
}

fn validate_watch_files(svc: &ServiceConfig) -> anyhow::Result<()> {
    let Some(w) = &svc.watch_files else {
        return Ok(());
    };
    anyhow::ensure!(
        svc.service_type != Some(vigil_types::plan::ServiceType::Oneshot),
        "watch-files does not apply to type: oneshot (every run reads its files anew)"
    );
    if let Some(i) = &w.interval {
        crate::duration::parse_interval(i)
            .map_err(|e| anyhow::anyhow!("watch-files interval {e}"))?;
    }
    for p in w.paths.iter().flatten() {
        anyhow::ensure!(
            p.starts_with('/'),
            "watch-files path {p:?} must be absolute"
        );
    }
    Ok(())
}

fn validate_oneshot(svc: &ServiceConfig) -> anyhow::Result<()> {
    use vigil_types::plan::{OnExit, ServiceType};

    let oneshot = svc.service_type == Some(ServiceType::Oneshot);
    if let Some(pattern) = &svc.schedule {
        anyhow::ensure!(oneshot, "schedule needs type: oneshot");
        crate::service::schedule::Schedule::parse(pattern, svc.schedule_tz.as_deref())?;
    } else {
        anyhow::ensure!(
            svc.schedule_tz.is_none(),
            "schedule-tz is only valid with schedule"
        );
    }
    if oneshot {
        anyhow::ensure!(
            svc.ready.is_none() && svc.ready_check.is_none() && svc.ready_timeout.is_none(),
            "ready, ready-check and ready-timeout do not apply to type: oneshot \
             (a oneshot is ready when it exits successfully)"
        );
        anyhow::ensure!(
            svc.on_success != Some(OnExit::Restart),
            "on-success: restart does not apply to type: oneshot \
             (use a schedule to run it repeatedly)"
        );
    }
    Ok(())
}

fn validate_ready(plan: &Plan, svc: &ServiceConfig) -> anyhow::Result<()> {
    use vigil_types::plan::{ReadyMode, Startup};

    parse_field("ready-timeout", svc.ready_timeout.as_deref())?;
    match (svc.ready.unwrap_or_default(), &svc.ready_check) {
        (ReadyMode::Check, None) => anyhow::bail!("ready: check needs ready-check"),
        (ReadyMode::Check, Some(c)) => {
            let check = plan
                .checks
                .get(c)
                .with_context(|| format!("ready-check {c:?} is not a defined check"))?;
            anyhow::ensure!(
                check.startup() != Startup::Disabled,
                "ready-check {c:?} has startup: disabled and would never run"
            );
        }
        (_, Some(_)) => anyhow::bail!("ready-check is only valid with ready: check"),
        (_, None) => {}
    }
    Ok(())
}
