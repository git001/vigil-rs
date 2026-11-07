// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Validation of a check, and the warning for alive checks of other hosts.

use anyhow::Context;
use vigil_types::plan::{CheckConfig, Plan};

use crate::process_util::{EnvPolicy, REDUNDANT_LISTS, redundant_lists};

use crate::duration::parse_field;

pub(super) fn validate_check(plan: &Plan, name: &str, check: &CheckConfig) -> anyhow::Result<()> {
    if let Some(p) = &check.period {
        crate::duration::parse_interval(p).map_err(|e| anyhow::anyhow!("period {e}"))?;
    }
    parse_field("delay", check.delay.as_deref())?;
    if let Some(t) = parse_field("timeout", check.timeout.as_deref())? {
        anyhow::ensure!(!t.is_zero(), "timeout must be greater than 0");
    }
    anyhow::ensure!(check.threshold != Some(0), "threshold must be at least 1");
    // A check without a probe would always pass; with several, only one
    // of them would run.
    let probes = [
        check.http.is_some(),
        check.tcp.is_some(),
        check.exec.is_some(),
    ];
    anyhow::ensure!(
        probes.iter().filter(|p| **p).count() == 1,
        "needs exactly one of http, tcp or exec"
    );
    if let Some(http) = &check.http {
        anyhow::ensure!(!http.url.is_empty(), "http: url must not be empty");
    }
    if let Some(s) = &check.service {
        anyhow::ensure!(
            plan.services.contains_key(s),
            "service {s:?} is not defined"
        );
    }
    if let Some(exec) = &check.exec {
        let ctx = match exec.service_context.as_deref() {
            None => None,
            Some(s) => Some(
                plan.services
                    .get(s)
                    .with_context(|| format!("exec: service-context {s:?} is not a service"))?,
            ),
        };
        EnvPolicy::for_exec_check(exec, ctx)?;
        // Only the check's own lists: a service context still needs its
        // lists for the service itself.
        let pass_env = exec.pass_env.or_else(|| ctx.and_then(|s| s.pass_env));
        if redundant_lists(pass_env, &exec.pass_env_glob, &exec.pass_env_regex) {
            tracing::warn!(check = %name, "{REDUNDANT_LISTS}");
        }
        crate::template::check_exec_environment(&exec.environment)?;
    }
    if let Some(warning) = level_warning(check) {
        tracing::warn!(check = %name, "{warning}");
    }
    Ok(())
}

/// A running `alive` check (explicit or by default) that probes another
/// host: it most likely tests a dependency. When it is down, a liveness
/// probe on `GET /v1/health?level=alive` restarts the container, which does
/// not fix the dependency. Exec checks are not inspected.
pub(super) fn level_warning(check: &CheckConfig) -> Option<String> {
    use vigil_types::plan::{CheckLevel, Startup};
    if check.level() != CheckLevel::Alive || check.startup() != Startup::Enabled {
        return None;
    }
    let target = if let Some(http) = &check.http {
        let url = reqwest::Url::parse(&http.url).ok()?;
        let host = url.host_str()?;
        if is_local_host(host) {
            return None;
        }
        host.to_owned()
    } else if let Some(tcp) = &check.tcp {
        let host = tcp.host.as_deref().unwrap_or("localhost");
        if is_local_host(host) {
            return None;
        }
        format!("{host}:{}", tcp.port)
    } else {
        return None;
    };
    let level = if check.level.is_some() {
        "level: alive"
    } else {
        "level: alive (the default)"
    };
    Some(format!(
        "{level}, but it probes {target:?}, not this container: while it is down, \
         GET /v1/health?level=alive answers 502 and a liveness probe on it restarts \
         the container, which does not fix a dependency; set level: ready for checks \
         of dependencies"
    ))
}

/// This container: a loopback or unspecified address (`[::1]` with or
/// without brackets), `localhost` or a name under `.localhost` (RFC 6761).
fn is_local_host(host: &str) -> bool {
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
        return ip.is_loopback() || ip.is_unspecified();
    }
    let name = host.trim_end_matches('.').to_ascii_lowercase();
    name == "localhost" || name.ends_with(".localhost")
}
