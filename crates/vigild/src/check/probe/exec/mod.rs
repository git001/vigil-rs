// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Exec checks: run `sh -c` as the check's user, pass on exit status 0.

use std::time::Duration;

use indexmap::IndexMap;
use tokio::time::{timeout, timeout_at};
use tracing::{debug, warn};
use vigil_types::plan::{ExecCheck, ServiceConfig};

use crate::process_util::{EnvPolicy, apply_credentials, apply_env, resolve_credentials};
use crate::template::{EnvLayer, Renderer, child_env, off_runtime};

use super::{ProbeResult, timed_out};

mod output;
use output::{OUTPUT_DRAIN, OutputTail, exit_reason, pipe_pair, wait_and_read};

pub(super) async fn probe_exec(
    exec: &ExecCheck,
    timeout_dur: Duration,
    service_configs: &IndexMap<String, ServiceConfig>,
) -> ProbeResult {
    let mut cmd = build_command(exec, service_configs).await?;
    // stdout and stderr share one pipe, so the tail keeps their order. It
    // is only reported when the check fails.
    let (reader, writer) = match pipe_pair() {
        Ok(p) => p,
        Err(e) => return Err(format!("cannot create the output pipe: {e}")),
    };
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(writer.0);
    cmd.stderr(writer.1);
    // Own process group: a timed-out check is killed with everything it
    // started (e.g. both sides of `curl … | grep …`).
    cmd.process_group(0);

    let spawned_at = std::time::Instant::now();
    let spawned = cmd.spawn();
    // Drop our copies of the write end, or the pipe never reports EOF.
    drop(cmd);
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            debug!(command = %exec.command, error = %e, "exec probe: process did not start");
            return Err(format!("cannot start sh: {e}"));
        }
    };

    let mut output = OutputTail::default();
    let mut reader = Some(reader);
    let deadline = tokio::time::Instant::now() + timeout_dur;
    let waited = timeout_at(
        deadline,
        wait_and_read(&mut child, spawned_at, &mut reader, &mut output),
    )
    .await;
    let result = match waited {
        Ok(Ok(status)) => {
            // Output still in the pipe after the exit; a background process
            // that keeps the pipe open must not stall the check.
            if let Some(r) = reader.as_mut() {
                let _ = timeout(OUTPUT_DRAIN, output.read_to_end(r)).await;
            }
            if status.success() {
                Ok(())
            } else {
                Err(output.append_to(exit_reason(status)))
            }
        }
        Ok(Err(e)) => Err(format!("waiting for the command failed: {e}")),
        Err(_) => {
            if let Some(pid) = child.id() {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            // Reap it; SIGKILL cannot be ignored, so this returns at once.
            let _ = crate::reaper::wait_child(&mut child, spawned_at).await;
            Err(output.append_to(timed_out(timeout_dur)))
        }
    };
    debug!(command = %exec.command, error = result.as_ref().err(), "exec probe");
    result
}

/// `sh -c <command>` with the user, group, working directory and
/// environment of the check, each falling back to its `service-context`.
async fn build_command(
    exec: &ExecCheck,
    service_configs: &IndexMap<String, ServiceConfig>,
) -> Result<tokio::process::Command, String> {
    // Resolve service-context: inherit env/user/group/working-dir from the
    // named service, then let check-specific settings override.
    let ctx_svc = exec
        .service_context
        .as_deref()
        .and_then(|n| service_configs.get(n));

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

    // Resolve the full credential set (fail-safe: log and fail the probe).
    let creds = match resolve_credentials(eff_user, eff_user_id, eff_group, eff_group_id) {
        Ok(c) => c,
        Err(e) => {
            warn!(%e, "exec check: failed to resolve user/group");
            return Err(format!("user/group: {e:#}"));
        }
    };

    // Same environment the service would get (see process_util::env).
    let policy = match EnvPolicy::for_exec_check(exec, ctx_svc) {
        Ok(p) => p,
        Err(e) => {
            warn!(%e, "exec check: invalid pass-env settings");
            return Err(format!("pass-env: {e:#}"));
        }
    };
    // Service context first (env-file, environment), the check's
    // environment on top; templates rendered as for the service, off the
    // async runtime (they may read files).
    let ctx_layer = ctx_svc.map(|s| (s.env_file.clone(), s.environment.clone()));
    let check_env = exec.environment.clone();
    let creds_for_env = creds.clone();
    let built = off_runtime("exec check environment", move || {
        let mut layers = Vec::with_capacity(2);
        if let Some((env_file, environment)) = &ctx_layer {
            layers.push(EnvLayer {
                env_file,
                environment,
            });
        }
        layers.push(EnvLayer {
            env_file: &[],
            environment: &check_env,
        });
        child_env(&Renderer::new(), &policy, creds_for_env.as_ref(), &layers)
    })
    .await;
    let env = match built {
        Ok(env) => env,
        Err(e) => {
            warn!(error = %e, "exec check: cannot build environment");
            return Err(format!("environment: {e:#}"));
        }
    };

    let mut cmd = tokio::process::Command::new("sh");
    cmd.args(["-c", &exec.command]);
    apply_env(&mut cmd, env);
    if let Some(dir) = eff_working_dir {
        cmd.current_dir(dir);
    }
    apply_credentials(&mut cmd, creds);
    Ok(cmd)
}
