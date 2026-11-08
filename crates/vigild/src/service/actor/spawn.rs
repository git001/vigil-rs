// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Process spawning — `Actor::do_start`.

use std::sync::Arc;

use tokio::process::Command;
use tracing::info;
use vigil_types::api::LogStream;
use vigil_types::plan::{LogsForward, ReadyMode};

use crate::logs::spawn_reader;
use crate::process_util::{
    EnvPolicy, apply_credentials, apply_env, parse_command_templated, resolve_credentials,
};
use crate::service::notify::NotifySocket;
use crate::template::{EnvLayer, Renderer, child_env, off_runtime, vars_of};

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

        let creds = resolve_credentials(
            self.config.user.as_deref(),
            self.config.user_id,
            self.config.group.as_deref(),
            self.config.group_id,
        )?;
        let policy = EnvPolicy::compile(
            self.config.pass_env,
            &self.config.pass_env_glob,
            &self.config.pass_env_regex,
        )?;
        // Templates never see vigild's whole environment, only what the
        // pass-env policy lets through plus env-file (see `child_env`);
        // `command` sees the child's final environment. Rendering reads
        // files, so it runs off the async runtime with a timeout.
        let cmd_str = cmd_str.to_owned();
        let env_file = self.config.env_file.clone();
        let environment = self.config.environment.clone();
        let creds_for_env = creds.clone();
        let (mut env, argv, reads) = off_runtime("templates", move || {
            let renderer = Renderer::new();
            let env = child_env(
                &renderer,
                &policy,
                creds_for_env.as_ref(),
                &[EnvLayer {
                    env_file: &env_file,
                    environment: &environment,
                }],
            )?;
            let vars = vars_of(&env);
            // A word that renders to nothing is dropped, like an empty
            // unquoted expansion in a shell, so `{% if … %}--flag{% endif %}`
            // adds an optional flag instead of an empty argument. (The
            // command parser never produces empty words itself.)
            let mut argv = Vec::new();
            for w in parse_command_templated(&cmd_str)? {
                let arg = renderer
                    .render(&w, &vars)
                    .map_err(|e| anyhow::anyhow!("command word {w:?}: {e}"))?;
                if !arg.is_empty() {
                    argv.push(arg);
                }
            }
            Ok((env, argv, renderer.take_reads()))
        })
        .await?;
        if argv.is_empty() {
            return Err(anyhow::anyhow!("empty command for service '{}'", self.name));
        }
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        if let Some(dir) = &self.config.working_dir {
            cmd.current_dir(dir);
        }

        // Never leak a NOTIFY_SOCKET vigild itself got (e.g. from systemd);
        // `ready: notify` services get their own one.
        env.retain(|(k, _)| k != "NOTIFY_SOCKET");
        self.service_uid = creds
            .as_ref()
            .map_or_else(nix::unistd::Uid::effective, |c| c.uid)
            .as_raw();
        let notify = if self.ready_mode() == ReadyMode::Notify {
            self.notify = None; // removes the previous start's socket
            self.notify_generation += 1;
            let sock = NotifySocket::bind(
                &self.notify_dir,
                &self.name,
                creds.as_ref().map(|c| (c.uid, c.gid)),
                self.notify_generation,
                self.notify_tx.clone(),
            )?;
            env.push(("NOTIFY_SOCKET".into(), sock.path().as_os_str().to_owned()));
            Some(sock)
        } else {
            None
        };
        apply_env(&mut cmd, env);
        apply_credentials(&mut cmd, creds);

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

        self.spawned_at = std::time::Instant::now();
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

        info!(service = %self.name, pid = child.id(), "process started");
        self.child = Some(child);
        self.notify = notify;
        self.start_watching(reads).await;
        self.metrics.record_service_start(&self.name);
        self.after_spawn().await;
        Ok(())
    }
}
