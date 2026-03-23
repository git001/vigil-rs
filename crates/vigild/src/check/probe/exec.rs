// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::time::Duration;

use indexmap::IndexMap;
use tokio::time::timeout;
use tracing::{debug, warn};
use vigil_types::plan::{ExecCheck, ServiceConfig};

use crate::process_util::{resolve_gid, resolve_uid};

pub(super) async fn probe_exec(
    exec: &ExecCheck,
    timeout_dur: Duration,
    service_configs: &IndexMap<String, ServiceConfig>,
) -> bool {
    // Resolve service-context: inherit env/user/group/working-dir from the
    // named service, then let check-specific settings override.
    let ctx_svc = exec
        .service_context
        .as_deref()
        .and_then(|n| service_configs.get(n));

    // Build effective environment: service env first, check env on top.
    let mut env: IndexMap<String, String> =
        ctx_svc.map(|s| s.environment.clone()).unwrap_or_default();
    env.extend(exec.environment.iter().map(|(k, v)| (k.clone(), v.clone())));

    // Effective user/group: check setting wins, fall back to service context.
    let eff_user = exec
        .user
        .as_deref()
        .or_else(|| ctx_svc.and_then(|s| s.user.as_deref()));
    let eff_user_id = exec.user_id.or_else(|| ctx_svc.and_then(|s| s.user_id));
    let eff_group = exec
        .group
        .as_deref()
        .or_else(|| ctx_svc.and_then(|s| s.group.as_deref()));
    let eff_group_id = exec.group_id.or_else(|| ctx_svc.and_then(|s| s.group_id));
    let eff_working_dir = exec
        .working_dir
        .as_deref()
        .or_else(|| ctx_svc.and_then(|s| s.working_dir.as_deref()));

    // Resolve uid/gid (fail-safe: log and skip on error).
    let uid = match resolve_uid(eff_user, eff_user_id) {
        Ok(u) => u,
        Err(e) => {
            warn!(%e, "exec check: failed to resolve user");
            return false;
        }
    };
    let gid = match resolve_gid(eff_group, eff_group_id) {
        Ok(g) => g,
        Err(e) => {
            warn!(%e, "exec check: failed to resolve group");
            return false;
        }
    };

    let command = exec.command.clone();
    let working_dir = eff_working_dir.map(str::to_owned);

    match timeout(timeout_dur, async move {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", &command]);

        if !env.is_empty() {
            cmd.envs(env.iter());
        }
        if let Some(dir) = &working_dir {
            cmd.current_dir(dir);
        }
        if uid.is_some() || gid.is_some() {
            unsafe {
                cmd.pre_exec(move || {
                    if let Some(g) = gid {
                        nix::unistd::setgid(g).map_err(|e| {
                            std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string())
                        })?;
                    }
                    if let Some(u) = uid {
                        nix::unistd::setuid(u).map_err(|e| {
                            std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string())
                        })?;
                    }
                    Ok(())
                });
            }
        }

        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        cmd.spawn().ok()?.wait().await.ok()
    })
    .await
    {
        Ok(Some(status)) => {
            let passed = status.success();
            debug!(command = %exec.command, exit_code = ?status.code(), passed, "exec probe");
            passed
        }
        Ok(None) => {
            debug!(command = %exec.command, "exec probe: process did not start");
            false
        }
        Err(_) => {
            debug!(command = %exec.command, "exec probe timed out");
            false
        }
    }
}
