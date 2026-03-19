// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::time::Sleep;
use tracing::{debug, error, info, warn};
use vigil_types::api::LogStream;
use vigil_types::plan::{OnExit, ServiceConfig};

use crate::duration::parse_duration;
use crate::logs::{spawn_reader, LogStore};
use crate::metrics::MetricsStore;
use crate::process_util::{parse_command, resolve_gid, resolve_uid};
use crate::state::ServiceState;

use super::{
    Cmd, Event, EventKind, Snapshot,
    DEFAULT_BACKOFF_DELAY, DEFAULT_BACKOFF_FACTOR, DEFAULT_BACKOFF_LIMIT, DEFAULT_KILL_DELAY,
};

// ---------------------------------------------------------------------------
// Actor
// ---------------------------------------------------------------------------

pub(super) struct Actor {
    name: String,
    config: ServiceConfig,
    state: ServiceState,
    since: DateTime<Utc>,
    child: Option<Child>,
    backoff_count: u32,
    current_backoff: Duration,
    event_tx: mpsc::Sender<Event>,
    log_store: Arc<LogStore>,
    metrics: Arc<MetricsStore>,
}

impl Actor {
    pub(super) fn new(
        name: String,
        config: ServiceConfig,
        event_tx: mpsc::Sender<Event>,
        log_store: Arc<LogStore>,
        metrics: Arc<MetricsStore>,
    ) -> Self {
        Actor {
            name,
            config,
            state: ServiceState::Inactive,
            since: Utc::now(),
            child: None,
            backoff_count: 0,
            current_backoff: DEFAULT_BACKOFF_DELAY,
            event_tx,
            log_store,
            metrics,
        }
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        Snapshot {
            name: self.name.clone(),
            state: self.state,
            since: self.since,
            startup: self.config.startup,
            pid: self.child.as_ref().and_then(|c| c.id()),
        }
    }

    async fn transition(&mut self, new_state: ServiceState) {
        debug!(service = %self.name, ?new_state, "state transition");
        self.state = new_state;
        self.since = Utc::now();
        self.metrics.set_service_active(&self.name, new_state == ServiceState::Active);
        let _ = self
            .event_tx
            .send(Event {
                service: self.name.clone(),
                kind: EventKind::StateChanged { new_state },
            })
            .await;
    }

    // -----------------------------------------------------------------------
    // Process spawning
    // -----------------------------------------------------------------------

    async fn do_start(&mut self) -> anyhow::Result<()> {
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
        if log_mode == vigil_types::plan::LogsForward::Passthrough {
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

        if log_mode != vigil_types::plan::LogsForward::Passthrough {
            let forward = log_mode == vigil_types::plan::LogsForward::Enabled;
            if let Some(stdout) = child.stdout.take() {
                spawn_reader(self.name.clone(), LogStream::Stdout, stdout, Arc::clone(&self.log_store), forward);
            }
            if let Some(stderr) = child.stderr.take() {
                spawn_reader(self.name.clone(), LogStream::Stderr, stderr, Arc::clone(&self.log_store), forward);
            }
        }

        info!(service = %self.name, pid = ?child.id(), "process started");
        self.child = Some(child);
        self.metrics.record_service_start(&self.name);
        self.transition(ServiceState::Active).await;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Signal helpers
    // -----------------------------------------------------------------------

    fn stop_signal(&self) -> Signal {
        use vigil_types::signal::StopSignal;
        self.config.stop_signal.unwrap_or(StopSignal::default()).0
    }

    fn kill_delay(&self) -> Duration {
        self.config
            .kill_delay
            .as_deref()
            .and_then(|s| parse_duration(s).ok())
            .unwrap_or(DEFAULT_KILL_DELAY)
    }

    fn send_stop_signal(&self) {
        if let Some(child) = &self.child {
            if let Some(pid) = child.id() {
                let pgid = Pid::from_raw(-(pid as i32));
                let sig = self.stop_signal();
                if let Err(e) = kill(pgid, sig) {
                    warn!(service = %self.name, error = %e, "kill(pgid) failed, retrying with pid");
                    let _ = kill(Pid::from_raw(pid as i32), sig);
                }
            }
        }
    }

    pub(super) fn send_signal(&self, signal: Signal) {
        if let Some(child) = &self.child {
            if let Some(pid) = child.id() {
                let pgid = Pid::from_raw(-(pid as i32));
                if let Err(e) = kill(pgid, signal) {
                    debug!(service = %self.name, error = %e, ?signal, "forward to pgid failed");
                    let _ = kill(Pid::from_raw(pid as i32), signal);
                }
            }
        }
    }

    fn send_sigkill(&self) {
        if let Some(child) = &self.child {
            if let Some(pid) = child.id() {
                let pgid = Pid::from_raw(-(pid as i32));
                if let Err(e) = kill(pgid, Signal::SIGKILL) {
                    // ESRCH = process already exited — expected on clean shutdown
                    if e != nix::errno::Errno::ESRCH {
                        warn!(service = %self.name, error = %e, "SIGKILL pgid failed");
                        let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Backoff helpers
    // -----------------------------------------------------------------------

    fn next_backoff(&mut self) -> Duration {
        let delay = self.current_backoff;

        let factor = self.config.backoff_factor.unwrap_or(DEFAULT_BACKOFF_FACTOR);
        let limit = self
            .config
            .backoff_limit
            .as_deref()
            .and_then(|s| parse_duration(s).ok())
            .unwrap_or(DEFAULT_BACKOFF_LIMIT);
        let base = self
            .config
            .backoff_delay
            .as_deref()
            .and_then(|s| parse_duration(s).ok())
            .unwrap_or(DEFAULT_BACKOFF_DELAY);

        let next = Duration::from_millis(
            ((self.current_backoff.as_millis() as f64) * factor) as u64,
        )
        .min(limit);

        self.current_backoff = if self.backoff_count == 0 { base } else { next };
        self.backoff_count += 1;
        delay
    }

    fn reset_backoff(&mut self) {
        self.backoff_count = 0;
        self.current_backoff = self
            .config
            .backoff_delay
            .as_deref()
            .and_then(|s| parse_duration(s).ok())
            .unwrap_or(DEFAULT_BACKOFF_DELAY);
    }

    fn backoff_limit_exceeded(&self) -> bool {
        let limit = self
            .config
            .backoff_limit
            .as_deref()
            .and_then(|s| parse_duration(s).ok())
            .unwrap_or(DEFAULT_BACKOFF_LIMIT);
        self.backoff_count > 0 && self.current_backoff >= limit
    }

    // -----------------------------------------------------------------------
    // Command handlers
    // -----------------------------------------------------------------------

    pub(super) async fn handle_start(
        &mut self,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
        stop_deadline: &mut Option<Pin<Box<Sleep>>>,
    ) {
        if self.state.is_running() {
            let _ = reply.send(Ok(()));
            return;
        }
        if self.state == ServiceState::Stopping {
            let _ = reply.send(Err(anyhow::anyhow!(
                "service '{}' is stopping, wait before starting",
                self.name
            )));
            return;
        }

        self.transition(ServiceState::Starting).await;
        match self.do_start().await {
            Ok(()) => {
                self.reset_backoff();
                *stop_deadline = None;
                let _ = reply.send(Ok(()));
            }
            Err(e) => {
                error!(service = %self.name, error = %e, "failed to start");
                self.transition(ServiceState::Error).await;
                let _ = reply.send(Err(e));
            }
        }
    }

    pub(super) async fn handle_stop(
        &mut self,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
        stop_deadline: &mut Option<Pin<Box<Sleep>>>,
        backoff_sleep: &mut Option<Pin<Box<Sleep>>>,
    ) {
        match self.state {
            ServiceState::Inactive | ServiceState::Error | ServiceState::Stopping => {
                let _ = reply.send(Ok(()));
                return;
            }
            ServiceState::Backoff => {
                *backoff_sleep = None;
                self.transition(ServiceState::Inactive).await;
                let _ = reply.send(Ok(()));
                return;
            }
            ServiceState::Starting | ServiceState::Active => {}
        }

        self.send_stop_signal();
        self.transition(ServiceState::Stopping).await;
        *stop_deadline = Some(Box::pin(tokio::time::sleep(self.kill_delay())));
        let _ = reply.send(Ok(()));
    }

    pub(super) async fn handle_restart(
        &mut self,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
        backoff_sleep: &mut Option<Pin<Box<Sleep>>>,
        stop_deadline: &mut Option<Pin<Box<Sleep>>>,
    ) {
        if self.state.is_running() || self.state == ServiceState::Stopping {
            self.send_stop_signal();
            self.transition(ServiceState::Stopping).await;
            *stop_deadline = Some(Box::pin(tokio::time::sleep(self.kill_delay())));
            *backoff_sleep = None;
            let _ = reply.send(Ok(()));
        } else {
            let (tx2, rx2) = tokio::sync::oneshot::channel();
            self.handle_start(tx2, stop_deadline).await;
            let res = rx2.await.unwrap_or(Ok(()));
            let _ = reply.send(res);
        }
    }

    pub(super) async fn handle_child_exit(
        &mut self,
        exit: std::io::Result<std::process::ExitStatus>,
        backoff_sleep: &mut Option<Pin<Box<Sleep>>>,
        stop_deadline: &mut Option<Pin<Box<Sleep>>>,
        pending_restart: &mut bool,
    ) {
        let exit_status = exit.as_ref().ok().copied();
        let success = exit_status.map(|s| s.success()).unwrap_or(false);
        let raw_exit_code = exit_status
            .and_then(|s| s.code())
            .unwrap_or(if success { 0 } else { 1 });

        let _ = self
            .event_tx
            .send(Event {
                service: self.name.clone(),
                kind: EventKind::ProcessExited { success },
            })
            .await;

        info!(service = %self.name, success, exit_code = raw_exit_code, "process exited");
        self.child = None;
        *stop_deadline = None;

        if self.state == ServiceState::Stopping {
            self.transition(ServiceState::Inactive).await;
            if *pending_restart {
                *pending_restart = false;
                self.transition(ServiceState::Starting).await;
                match self.do_start().await {
                    Ok(()) => self.reset_backoff(),
                    Err(e) => {
                        error!(service = %self.name, error = %e, "restart failed");
                        self.transition(ServiceState::Error).await;
                    }
                }
            }
            return;
        }

        let policy = if success { self.config.on_success } else { self.config.on_failure };

        match policy {
            Some(OnExit::Ignore) => {
                self.transition(ServiceState::Inactive).await;
            }
            Some(OnExit::Shutdown) => {
                info!(service = %self.name, exit_code = raw_exit_code, "shutdown requested");
                self.transition(ServiceState::Inactive).await;
                let _ = self.event_tx.send(Event {
                    service: self.name.clone(),
                    kind: EventKind::DaemonShutdown { exit_code: raw_exit_code },
                }).await;
            }
            Some(OnExit::FailureShutdown) => {
                info!(service = %self.name, exit_code = raw_exit_code, "failure-shutdown requested");
                self.transition(ServiceState::Inactive).await;
                let _ = self.event_tx.send(Event {
                    service: self.name.clone(),
                    kind: EventKind::DaemonShutdown { exit_code: raw_exit_code },
                }).await;
            }
            Some(OnExit::SuccessShutdown) => {
                info!(service = %self.name, exit_code = raw_exit_code, "success-shutdown requested");
                self.transition(ServiceState::Inactive).await;
                let _ = self.event_tx.send(Event {
                    service: self.name.clone(),
                    kind: EventKind::DaemonShutdown { exit_code: raw_exit_code },
                }).await;
            }
            Some(OnExit::Restart) | None => {
                if self.backoff_limit_exceeded() {
                    error!(service = %self.name, "backoff limit exceeded, giving up");
                    self.transition(ServiceState::Error).await;
                } else {
                    let delay = self.next_backoff();
                    info!(service = %self.name, ?delay, "scheduling restart (backoff)");
                    self.transition(ServiceState::Backoff).await;
                    *backoff_sleep = Some(Box::pin(tokio::time::sleep(delay)));
                }
            }
        }
    }

    pub(super) async fn handle_kill_deadline(&mut self) {
        if self.child.is_some() {
            warn!(service = %self.name, "kill-delay expired, sending SIGKILL");
            self.send_sigkill();
        }
    }

    pub(super) async fn handle_backoff_expired(
        &mut self,
        stop_deadline: &mut Option<Pin<Box<Sleep>>>,
    ) {
        info!(service = %self.name, "backoff elapsed, restarting");
        self.transition(ServiceState::Starting).await;
        match self.do_start().await {
            Ok(()) => {}
            Err(e) => {
                error!(service = %self.name, error = %e, "restart after backoff failed");
                self.transition(ServiceState::Error).await;
            }
        }
        *stop_deadline = None;
    }

    pub(super) async fn cleanup(&mut self) {
        if self.child.is_some() {
            self.send_stop_signal();
            tokio::time::sleep(Duration::from_millis(500)).await;
            self.send_sigkill();
            if let Some(mut c) = self.child.take() {
                let _ = c.wait().await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Main actor loop
// ---------------------------------------------------------------------------

pub(super) async fn run(
    name: String,
    config: ServiceConfig,
    mut rx: mpsc::Receiver<Cmd>,
    event_tx: mpsc::Sender<Event>,
    log_store: Arc<LogStore>,
    metrics: Arc<MetricsStore>,
) {
    let mut actor = Actor::new(name.clone(), config, event_tx, Arc::clone(&log_store), metrics);
    let mut backoff_sleep: Option<Pin<Box<Sleep>>> = None;
    let mut stop_deadline: Option<Pin<Box<Sleep>>> = None;
    let mut pending_restart = false;

    // Spawn log-push tasks if the service config requests it.
    // Each task connects to the target with exponential-backoff retry and
    // streams ndjson log entries for this service. Handles are aborted when
    // the actor exits so the tasks stop immediately on shutdown / replan.
    let mut push_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    if let Some(path) = actor.config.logs_push_socket.clone() {
        push_tasks.push(crate::logs::spawn_push_unix(
            name.clone(), path, Arc::clone(&log_store),
        ));
    }
    if let Some(addr) = actor.config.logs_push_addr.clone() {
        push_tasks.push(crate::logs::spawn_push_tcp(
            name.clone(), addr, Arc::clone(&log_store),
        ));
    }

    loop {
        tokio::select! {
            biased;

            cmd = rx.recv() => {
                match cmd {
                    None | Some(Cmd::Shutdown) => break,
                    Some(Cmd::Start(reply)) => {
                        actor.handle_start(reply, &mut stop_deadline).await;
                    }
                    Some(Cmd::Stop(reply)) => {
                        actor.handle_stop(reply, &mut stop_deadline, &mut backoff_sleep).await;
                    }
                    Some(Cmd::Restart(reply)) => {
                        pending_restart = true;
                        actor.handle_restart(reply, &mut backoff_sleep, &mut stop_deadline).await;
                    }
                    Some(Cmd::Status(reply)) => {
                        let _ = reply.send(actor.snapshot());
                    }
                    Some(Cmd::ForwardSignal(sig)) => {
                        actor.send_signal(sig);
                    }
                }
            }

            status = async {
                match actor.child.as_mut() {
                    Some(c) => c.wait().await,
                    None => std::future::pending().await,
                }
            } => {
                actor.handle_child_exit(status, &mut backoff_sleep, &mut stop_deadline, &mut pending_restart).await;
            }

            _ = async {
                match stop_deadline.as_mut() {
                    Some(s) => s.await,
                    None => std::future::pending().await,
                }
            }, if stop_deadline.is_some() => {
                actor.handle_kill_deadline().await;
                stop_deadline = None;
            }

            _ = async {
                match backoff_sleep.as_mut() {
                    Some(s) => s.await,
                    None => std::future::pending().await,
                }
            }, if backoff_sleep.is_some() => {
                actor.handle_backoff_expired(&mut stop_deadline).await;
                backoff_sleep = None;
            }
        }
    }

    for h in push_tasks {
        h.abort();
    }
    actor.cleanup().await;
    debug!(service = %name, "actor shut down");
}
