// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The actor loop: commands, notify messages, the child's exit and the
//! deadlines.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, info};
use vigil_types::plan::ServiceConfig;

use crate::logs::LogStore;

use super::super::{Cmd, Context};
use super::Actor;

pub(in crate::service) async fn run(
    name: String,
    config: ServiceConfig,
    mut rx: mpsc::Receiver<Cmd>,
    ctx: Context,
) {
    let log_store = Arc::clone(&ctx.log_store);
    let mut actor = Actor::new(name.clone(), config, ctx);
    let mut notify_rx = actor.notify_rx.take().expect("fresh actor");
    actor.arm_schedule().await;

    let push_tasks = spawn_log_push(&name, &actor.config, &log_store);

    loop {
        if actor.child.is_none() {
            for waiter in actor.stop_waiters.drain(..) {
                let _ = waiter.send(());
            }
        }
        // Copies: the arms below borrow the actor mutably.
        let stop_deadline = actor.stop_deadline;
        let backoff_deadline = actor.backoff_deadline;
        let ready_deadline = actor.ready_deadline;
        let watch_deadline = actor.watch_deadline;
        let healthy_deadline = actor.healthy_deadline;
        let next_run = actor.next_run_deadline();
        tokio::select! {
            biased;

            cmd = rx.recv() => {
                match cmd {
                    None | Some(Cmd::Shutdown) => break,
                    Some(Cmd::Start(reply)) => {
                        let _ = reply.send(actor.try_start().await);
                    }
                    Some(Cmd::Stop(reply)) => {
                        actor.stop().await;
                        let _ = reply.send(Ok(()));
                    }
                    Some(Cmd::StopWait(reply)) => {
                        actor.stop().await;
                        // Answered at the top of the loop once no process runs.
                        actor.stop_waiters.push(reply);
                    }
                    Some(Cmd::Restart(reply)) => {
                        let _ = reply.send(actor.restart().await);
                    }
                    #[cfg(test)]
                    Some(Cmd::Status(reply)) => {
                        let _ = reply.send(actor.snapshot());
                    }
                    Some(Cmd::ForwardSignal(sig)) => {
                        actor.send_signal(sig);
                    }
                    Some(Cmd::Signal(sig, reply)) => {
                        let running = actor.child.is_some();
                        if running {
                            info!(service = %actor.name, signal = %sig, "sending signal");
                            actor.send_signal(sig);
                        }
                        let _ = reply.send(running);
                    }
                    Some(Cmd::UpdateConfig(config)) => {
                        actor.handle_update_config(*config).await;
                    }
                    Some(Cmd::CheckPassed { check, started_at }) => {
                        actor.handle_check_passed(&check, started_at).await;
                    }
                }
            }

            Some(msg) = notify_rx.recv() => {
                actor.handle_notify(msg).await;
            }

            status = async {
                match actor.child.as_mut() {
                    Some(c) => crate::reaper::wait_child(c, actor.spawned_at).await,
                    None => std::future::pending().await,
                }
            } => {
                actor.handle_child_exit(status).await;
            }

            _ = sleep_opt(stop_deadline), if stop_deadline.is_some() => {
                actor.stop_deadline = None;
                actor.handle_kill_deadline().await;
            }

            _ = sleep_opt(backoff_deadline), if backoff_deadline.is_some() => {
                actor.backoff_deadline = None;
                actor.handle_backoff_expired().await;
            }

            _ = sleep_opt(ready_deadline), if ready_deadline.is_some() => {
                actor.handle_ready_timeout().await;
            }

            _ = sleep_opt(watch_deadline), if watch_deadline.is_some() => {
                actor.handle_watch_tick().await;
            }

            _ = sleep_opt(healthy_deadline), if healthy_deadline.is_some() => {
                actor.report_healthy().await;
            }

            _ = sleep_opt(next_run), if next_run.is_some() => {
                actor.handle_schedule_due().await;
            }
        }
    }

    for h in push_tasks {
        h.abort();
    }
    actor.cleanup().await;
    debug!(service = %name, "actor shut down");
}

/// Sleep until `deadline`; never returns without one (for `select!` arms
/// guarded by `deadline.is_some()`).
async fn sleep_opt(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

/// Spawn the log-push tasks the config asks for (`logs-push-socket`,
/// `logs-push-addr`). Each connects to its target with exponential-backoff
/// retry and streams ndjson log entries of this service. The caller aborts
/// them when the actor exits, so they stop at once on shutdown or replan.
fn spawn_log_push(
    name: &str,
    config: &ServiceConfig,
    log_store: &Arc<LogStore>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut tasks = Vec::new();
    if let Some(path) = config.logs_push_socket.clone() {
        tasks.push(crate::logs::spawn_push_unix(
            name.to_owned(),
            path,
            Arc::clone(log_store),
        ));
    }
    if let Some(addr) = config.logs_push_addr.clone() {
        tasks.push(crate::logs::spawn_push_tcp(
            name.to_owned(),
            addr,
            Arc::clone(log_store),
        ));
    }
    tasks
}
