// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Process spawning — `Actor::do_start`.

use std::sync::Arc;

use tokio::process::Command;
use tracing::info;
use vigil_types::api::LogStream;
use vigil_types::plan::LogsForward;

use crate::logs::spawn_reader;
use crate::process_util::{parse_command, resolve_gid, resolve_uid};
use crate::state::ServiceState;

use super::Actor;

impl Actor {
    // -----------------------------------------------------------------------
    // Process spawning
    // -----------------------------------------------------------------------

    pub(super) async fn do_start(&mut self) -> anyhow::Result<()> {
        let cmd_str = self
            .config
            .command
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("service '{}' has no command", self.name))?;

        let argv = parse_command(cmd_str)?;
        if argv.is_empty() {
            return Err(anyhow::anyhow!("empty command for service '{}'", self.name));
        }

        let mut cmd = Command::new(&argv[0]);
        if argv.len() > 1 {
            cmd.args(&argv[1..]);
        }

        if !self.config.environment.is_empty() {
            cmd.envs(self.config.environment.iter());
        }
        if let Some(dir) = &self.config.working_dir {
            cmd.current_dir(dir);
        }

        let uid = resolve_uid(self.config.user.as_deref(), self.config.user_id)?;
        let gid = resolve_gid(self.config.group.as_deref(), self.config.group_id)?;
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

        cmd.process_group(0);

        let log_mode = self.config.logs_forward.unwrap_or_default();
        if log_mode == LogsForward::Passthrough {
            // Let the service's stdout/stderr pass through directly to the
            // container stdout/stderr. vigild does not capture or buffer
            // anything — the process writes directly to fd 1 / fd 2.
            cmd.stdout(std::process::Stdio::inherit());
            cmd.stderr(std::process::Stdio::inherit());
        } else {
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn '{}': {}", argv[0], e))?;

        if log_mode != LogsForward::Passthrough {
            let forward = log_mode == LogsForward::Enabled;
            if let Some(stdout) = child.stdout.take() {
                spawn_reader(
                    self.name.clone(),
                    LogStream::Stdout,
                    stdout,
                    Arc::clone(&self.log_store),
                    forward,
                );
            }
            if let Some(stderr) = child.stderr.take() {
                spawn_reader(
                    self.name.clone(),
                    LogStream::Stderr,
                    stderr,
                    Arc::clone(&self.log_store),
                    forward,
                );
            }
        }

        info!(service = %self.name, pid = ?child.id(), "process started");
        self.child = Some(child);
        self.metrics.record_service_start(&self.name);
        self.transition(ServiceState::Active).await;
        Ok(())
    }
}
