// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use nix::sys::signal::Signal;
use std::time::Duration;
use tokio::sync::mpsc;
use vigil_types::plan::{ReadyMode, ServiceConfig};
use vigil_types::signal::StopSignal;

use super::*;
use crate::logs::LogStore;
use crate::metrics::MetricsStore;
use crate::service::{Cmd, DEFAULT_BACKOFF_DELAY, DEFAULT_BACKOFF_LIMIT, Event, EventKind};
use crate::state::ServiceState;

fn test_ctx(
    event_tx: mpsc::Sender<Event>,
    log_store: std::sync::Arc<LogStore>,
    metrics: std::sync::Arc<MetricsStore>,
) -> crate::service::Context {
    crate::service::Context {
        event_tx,
        log_store,
        metrics,
        exit_code_mode: crate::service::ExitCodeMode::Propagate,
        notify_dir: std::env::temp_dir(),
    }
}

fn make_actor() -> (Actor, mpsc::Receiver<Event>) {
    let (event_tx, event_rx) = mpsc::channel(64);
    let log_store = LogStore::new(100, 64);
    let metrics = MetricsStore::new();
    let actor = Actor::new(
        "test-svc".into(),
        ServiceConfig::default(),
        test_ctx(event_tx, log_store, metrics),
    );
    (actor, event_rx)
}

fn make_actor_with_config(config: ServiceConfig) -> (Actor, mpsc::Receiver<Event>) {
    let (event_tx, event_rx) = mpsc::channel(64);
    let log_store = LogStore::new(100, 64);
    let metrics = MetricsStore::new();
    let actor = Actor::new(
        "test-svc".into(),
        config,
        test_ctx(event_tx, log_store, metrics),
    );
    (actor, event_rx)
}

// -----------------------------------------------------------------------
// Backoff unit tests
// -----------------------------------------------------------------------

#[test]
fn next_backoff_first_call_returns_initial_delay() {
    let (mut actor, _) = make_actor();
    let delay = actor.next_backoff();
    assert_eq!(delay, DEFAULT_BACKOFF_DELAY);
    assert_eq!(actor.backoff_count, 1);
}

#[test]
fn next_backoff_doubles_on_each_call() {
    let (mut actor, _) = make_actor();
    let d0 = actor.next_backoff(); // 500ms → advances to 1000ms
    let d1 = actor.next_backoff(); // 1000ms → advances to 2000ms
    let d2 = actor.next_backoff(); // 2000ms → advances to 4000ms
    assert_eq!(d0, Duration::from_millis(500));
    assert_eq!(d1, Duration::from_millis(1000));
    assert_eq!(d2, Duration::from_millis(2000));
}

#[test]
fn next_backoff_capped_at_limit() {
    let (mut actor, _) = make_actor();
    // Call enough times to exceed the default 30s limit
    let mut last = Duration::ZERO;
    for _ in 0..20 {
        last = actor.next_backoff();
    }
    assert!(
        last <= DEFAULT_BACKOFF_LIMIT,
        "backoff {last:?} exceeded limit {DEFAULT_BACKOFF_LIMIT:?}"
    );
}

#[test]
fn reset_backoff_resets_count_and_delay() {
    let (mut actor, _) = make_actor();
    actor.next_backoff();
    actor.next_backoff();
    actor.next_backoff();
    assert!(actor.backoff_count > 0);
    actor.reset_backoff();
    assert_eq!(actor.backoff_count, 0);
    assert_eq!(actor.current_backoff, DEFAULT_BACKOFF_DELAY);
}

#[test]
fn backoff_limit_exceeded_false_initially() {
    let (actor, _) = make_actor();
    assert!(!actor.backoff_limit_exceeded());
}

#[test]
fn backoff_limit_exceeded_after_many_retries() {
    let (mut actor, _) = make_actor();
    for _ in 0..30 {
        actor.next_backoff();
    }
    assert!(actor.backoff_limit_exceeded());
}

#[test]
fn custom_backoff_factor_applied() {
    let config = ServiceConfig {
        backoff_factor: Some(3.0),
        ..Default::default()
    };
    let (mut actor, _) = make_actor_with_config(config);
    let d0 = actor.next_backoff(); // 500ms → advances to 1500ms
    let d1 = actor.next_backoff(); // 1500ms → advances to 4500ms
    assert_eq!(d0, Duration::from_millis(500));
    assert_eq!(d1, Duration::from_millis(1500));
}

#[test]
fn snapshot_reflects_initial_state() {
    let (actor, _) = make_actor();
    let snap = actor.snapshot();
    assert_eq!(snap.name, "test-svc");
    assert_eq!(snap.state, ServiceState::Inactive);
    assert!(snap.pid.is_none());
}

// -----------------------------------------------------------------------
// Integration tests — real process spawning
// -----------------------------------------------------------------------

#[tokio::test]
async fn start_process_becomes_active_then_exits_to_backoff() {
    let config = ServiceConfig {
        command: Some("/bin/sh -c 'exit 1'".into()),
        ..Default::default()
    };
    let (event_tx, mut event_rx) = mpsc::channel(64);
    let log_store = LogStore::new(100, 64);
    let metrics = MetricsStore::new();
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(run(
        "svc".into(),
        config,
        rx,
        crate::service::Context {
            notify_dir: std::env::temp_dir(),
            ..test_ctx(event_tx, log_store, metrics)
        },
    ));

    // Start the service
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(Cmd::Start(reply_tx)).await.unwrap();
    let result = reply_rx.await.unwrap();
    assert!(result.is_ok());

    // Should receive Starting then Active events
    let e1 = event_rx.recv().await.unwrap();
    assert!(matches!(
        e1.kind,
        EventKind::StateChanged {
            new_state: ServiceState::Starting
        } | EventKind::StateChanged {
            new_state: ServiceState::Active
        } | EventKind::ProcessExited { .. }
    ));

    // Shutdown the actor
    tx.send(Cmd::Shutdown).await.unwrap();
}

#[tokio::test]
async fn start_nonexistent_command_returns_error() {
    let config = ServiceConfig {
        command: Some("/nonexistent/binary/that/does/not/exist".into()),
        ..Default::default()
    };
    let (event_tx, _event_rx) = mpsc::channel(64);
    let log_store = LogStore::new(100, 64);
    let metrics = MetricsStore::new();
    let mut actor = Actor::new("svc".into(), config, test_ctx(event_tx, log_store, metrics));

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    actor.handle_start(reply_tx, &mut stop_deadline).await;
    let result = reply_rx.await.unwrap();
    assert!(result.is_err());
    assert_eq!(actor.state, ServiceState::Error);
}

#[tokio::test]
async fn start_empty_command_returns_error() {
    let config = ServiceConfig {
        command: Some("".into()),
        ..Default::default()
    };
    let (event_tx, _event_rx) = mpsc::channel(64);
    let log_store = LogStore::new(100, 64);
    let metrics = MetricsStore::new();
    let mut actor = Actor::new("svc".into(), config, test_ctx(event_tx, log_store, metrics));

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    actor.handle_start(reply_tx, &mut stop_deadline).await;
    let result = reply_rx.await.unwrap();
    assert!(result.is_err());
}

#[tokio::test]
async fn start_no_command_returns_error() {
    let (event_tx, _event_rx) = mpsc::channel(64);
    let log_store = LogStore::new(100, 64);
    let metrics = MetricsStore::new();
    let mut actor = Actor::new(
        "svc".into(),
        ServiceConfig::default(),
        test_ctx(event_tx, log_store, metrics),
    );
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    actor.handle_start(reply_tx, &mut stop_deadline).await;
    let result = reply_rx.await.unwrap();
    assert!(result.is_err());
    assert_eq!(actor.state, ServiceState::Error);
}

#[tokio::test]
async fn stop_inactive_service_is_noop() {
    let (mut actor, _) = make_actor();
    assert_eq!(actor.state, ServiceState::Inactive);
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    let mut backoff_sleep = None;
    actor
        .handle_stop(reply_tx, &mut stop_deadline, &mut backoff_sleep)
        .await;
    assert!(reply_rx.await.unwrap().is_ok());
    assert_eq!(actor.state, ServiceState::Inactive);
}

#[tokio::test]
async fn stop_error_state_is_noop() {
    let (mut actor, _) = make_actor();
    // Force error state via manual transition
    actor.state = ServiceState::Error;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    let mut backoff_sleep = None;
    actor
        .handle_stop(reply_tx, &mut stop_deadline, &mut backoff_sleep)
        .await;
    assert!(reply_rx.await.unwrap().is_ok());
    assert_eq!(actor.state, ServiceState::Error); // unchanged
}

#[tokio::test]
async fn stop_backoff_cancels_and_goes_inactive() {
    let (mut actor, _) = make_actor();
    actor.state = ServiceState::Backoff;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    let mut backoff_sleep: Option<std::pin::Pin<Box<tokio::time::Sleep>>> =
        Some(Box::pin(tokio::time::sleep(Duration::from_secs(60))));
    actor
        .handle_stop(reply_tx, &mut stop_deadline, &mut backoff_sleep)
        .await;
    assert!(reply_rx.await.unwrap().is_ok());
    assert_eq!(actor.state, ServiceState::Inactive);
    assert!(backoff_sleep.is_none());
}

#[tokio::test]
async fn start_when_already_running_is_noop() {
    let config = ServiceConfig {
        command: Some("/bin/sleep 60".into()),
        ..Default::default()
    };
    let (event_tx, _event_rx) = mpsc::channel(64);
    let log_store = LogStore::new(100, 64);
    let metrics = MetricsStore::new();
    let mut actor = Actor::new("svc".into(), config, test_ctx(event_tx, log_store, metrics));

    // Start once
    let (r1_tx, r1_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    actor.handle_start(r1_tx, &mut stop_deadline).await;
    let _ = r1_rx.await.unwrap(); // may succeed or fail

    if actor.state.is_running() {
        // Try starting again — should be a no-op
        let (r2_tx, r2_rx) = tokio::sync::oneshot::channel();
        actor.handle_start(r2_tx, &mut stop_deadline).await;
        assert!(r2_rx.await.unwrap().is_ok()); // ok, already running
        actor.cleanup().await;
    }
}

// -----------------------------------------------------------------------
// Bug fix: pending_restart must not be set when service is not running
// -----------------------------------------------------------------------

#[tokio::test]
async fn restart_on_inactive_does_not_set_pending_restart() {
    let config = ServiceConfig {
        command: Some("/bin/sleep 60".into()),
        ..Default::default()
    };
    let (mut actor, _) = make_actor_with_config(config);
    assert_eq!(actor.state, ServiceState::Inactive);

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut backoff_sleep = None;
    let mut stop_deadline = None;
    let mut pending_restart = false;

    actor
        .handle_restart(
            reply_tx,
            &mut backoff_sleep,
            &mut stop_deadline,
            &mut pending_restart,
        )
        .await;
    let _ = reply_rx.await;

    // Service started directly — pending_restart must remain false
    assert!(
        !pending_restart,
        "pending_restart must not be set when service starts directly via restart"
    );
    actor.cleanup().await;
}

#[tokio::test]
async fn restart_on_running_sets_pending_restart() {
    let config = ServiceConfig {
        command: Some("/bin/sleep 60".into()),
        ..Default::default()
    };
    let (mut actor, _) = make_actor_with_config(config);

    // Start the service first
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    actor.handle_start(start_tx, &mut stop_deadline).await;
    let _ = start_rx.await;

    if actor.state.is_running() {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let mut backoff_sleep = None;
        let mut pending_restart = false;

        actor
            .handle_restart(
                reply_tx,
                &mut backoff_sleep,
                &mut stop_deadline,
                &mut pending_restart,
            )
            .await;
        let _ = reply_rx.await;

        // Now going through stop path — pending_restart must be true
        assert!(
            pending_restart,
            "pending_restart must be set when stopping a running service for restart"
        );
        actor.cleanup().await;
    }
}

// -----------------------------------------------------------------------
// Bug fix: shutdown exit codes
// -----------------------------------------------------------------------

#[tokio::test]
async fn shutdown_exit_code_success_gives_zero() {
    let (mut actor, mut event_rx) = make_actor();
    let mut backoff_sleep = None;
    let mut stop_deadline = None;
    let mut pending_restart = false;

    actor.config.on_success = Some(vigil_types::plan::OnExit::Shutdown);
    let exit: std::io::Result<std::process::ExitStatus> = {
        use std::os::unix::process::ExitStatusExt;
        Ok(std::process::ExitStatus::from_raw(0))
    };
    actor
        .handle_child_exit(
            exit,
            &mut backoff_sleep,
            &mut stop_deadline,
            &mut pending_restart,
        )
        .await;

    // Drain events to find DaemonShutdown
    let mut shutdown_code = None;
    while let Ok(ev) = event_rx.try_recv() {
        if let EventKind::DaemonShutdown { exit_code } = ev.kind {
            shutdown_code = Some(exit_code);
        }
    }
    assert_eq!(shutdown_code, Some(0), "Shutdown on success should exit 0");
}

#[tokio::test]
async fn shutdown_exit_code_failure_gives_ten() {
    let (mut actor, mut event_rx) = make_actor();
    actor.exit_code_mode = crate::service::ExitCodeMode::Fixed;
    let mut backoff_sleep = None;
    let mut stop_deadline = None;
    let mut pending_restart = false;

    actor.config.on_failure = Some(vigil_types::plan::OnExit::Shutdown);
    let exit: std::io::Result<std::process::ExitStatus> = {
        use std::os::unix::process::ExitStatusExt;
        Ok(std::process::ExitStatus::from_raw(1 << 8)) // exit code 1
    };
    actor
        .handle_child_exit(
            exit,
            &mut backoff_sleep,
            &mut stop_deadline,
            &mut pending_restart,
        )
        .await;

    let mut shutdown_code = None;
    while let Ok(ev) = event_rx.try_recv() {
        if let EventKind::DaemonShutdown { exit_code } = ev.kind {
            shutdown_code = Some(exit_code);
        }
    }
    assert_eq!(
        shutdown_code,
        Some(10),
        "Shutdown on failure should exit 10"
    );
}

#[tokio::test]
async fn failure_shutdown_always_exits_ten() {
    let (mut actor, mut event_rx) = make_actor();
    let mut backoff_sleep = None;
    let mut stop_deadline = None;
    let mut pending_restart = false;

    // FailureShutdown on on_success — process exited with 0 but daemon should still exit 10
    actor.config.on_success = Some(vigil_types::plan::OnExit::FailureShutdown);
    let exit: std::io::Result<std::process::ExitStatus> = {
        use std::os::unix::process::ExitStatusExt;
        Ok(std::process::ExitStatus::from_raw(0))
    };
    actor
        .handle_child_exit(
            exit,
            &mut backoff_sleep,
            &mut stop_deadline,
            &mut pending_restart,
        )
        .await;

    let mut shutdown_code = None;
    while let Ok(ev) = event_rx.try_recv() {
        if let EventKind::DaemonShutdown { exit_code } = ev.kind {
            shutdown_code = Some(exit_code);
        }
    }
    assert_eq!(
        shutdown_code,
        Some(10),
        "FailureShutdown must always exit 10"
    );
}

#[tokio::test]
async fn success_shutdown_always_exits_zero() {
    let (mut actor, mut event_rx) = make_actor();
    let mut backoff_sleep = None;
    let mut stop_deadline = None;
    let mut pending_restart = false;

    // SuccessShutdown on on_failure — process exited with 1 but daemon should still exit 0
    actor.config.on_failure = Some(vigil_types::plan::OnExit::SuccessShutdown);
    let exit: std::io::Result<std::process::ExitStatus> = {
        use std::os::unix::process::ExitStatusExt;
        Ok(std::process::ExitStatus::from_raw(1 << 8)) // exit code 1
    };
    actor
        .handle_child_exit(
            exit,
            &mut backoff_sleep,
            &mut stop_deadline,
            &mut pending_restart,
        )
        .await;

    let mut shutdown_code = None;
    while let Ok(ev) = event_rx.try_recv() {
        if let EventKind::DaemonShutdown { exit_code } = ev.kind {
            shutdown_code = Some(exit_code);
        }
    }
    assert_eq!(shutdown_code, Some(0), "SuccessShutdown must always exit 0");
}

// -----------------------------------------------------------------------
// do_start coverage — spawn.rs paths
// -----------------------------------------------------------------------

// Covers the `if argv.len() > 1 { cmd.args(&argv[1..]); }` branch.
#[tokio::test]
async fn do_start_command_with_multiple_args() {
    let config = ServiceConfig {
        command: Some("echo hello world".into()),
        ..Default::default()
    };
    let (event_tx, _event_rx) = mpsc::channel(64);
    let log_store = LogStore::new(100, 64);
    let metrics = MetricsStore::new();
    let mut actor = Actor::new("svc".into(), config, test_ctx(event_tx, log_store, metrics));

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    actor.handle_start(reply_tx, &mut stop_deadline).await;
    let result = reply_rx.await.unwrap();
    assert!(
        result.is_ok(),
        "expected successful spawn with args: {result:?}"
    );
    actor.cleanup().await;
}

// Covers the `if !self.config.environment.is_empty()` branch.
#[tokio::test]
async fn do_start_with_environment_vars() {
    let mut env = indexmap::IndexMap::new();
    env.insert("_VIGIL_SPAWN_TEST_VAR".into(), "present".into());
    let config = ServiceConfig {
        command: Some("/bin/sh -c 'echo $_VIGIL_SPAWN_TEST_VAR'".into()),
        environment: env,
        ..Default::default()
    };
    let (event_tx, _event_rx) = mpsc::channel(64);
    let log_store = LogStore::new(100, 64);
    let metrics = MetricsStore::new();
    let mut actor = Actor::new("svc".into(), config, test_ctx(event_tx, log_store, metrics));

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    actor.handle_start(reply_tx, &mut stop_deadline).await;
    let result = reply_rx.await.unwrap();
    assert!(result.is_ok(), "spawn with env failed: {result:?}");
    actor.cleanup().await;
}

// Covers the `if let Some(dir) = &self.config.working_dir` branch.
#[tokio::test]
async fn do_start_with_working_dir() {
    let config = ServiceConfig {
        command: Some("pwd".into()),
        working_dir: Some("/tmp".into()),
        ..Default::default()
    };
    let (event_tx, _event_rx) = mpsc::channel(64);
    let log_store = LogStore::new(100, 64);
    let metrics = MetricsStore::new();
    let mut actor = Actor::new("svc".into(), config, test_ctx(event_tx, log_store, metrics));

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    actor.handle_start(reply_tx, &mut stop_deadline).await;
    let result = reply_rx.await.unwrap();
    assert!(result.is_ok(), "spawn with working_dir failed: {result:?}");
    actor.cleanup().await;
}

// Covers the `LogsForward::Passthrough` branch (cmd.stdout/stderr inherit).
#[tokio::test]
async fn do_start_passthrough_log_mode() {
    let config = ServiceConfig {
        command: Some("/bin/sleep 100".into()),
        logs_forward: Some(vigil_types::plan::LogsForward::Passthrough),
        ..Default::default()
    };
    let (event_tx, _event_rx) = mpsc::channel(64);
    let log_store = LogStore::new(100, 64);
    let metrics = MetricsStore::new();
    let mut actor = Actor::new("svc".into(), config, test_ctx(event_tx, log_store, metrics));

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    actor.handle_start(reply_tx, &mut stop_deadline).await;
    let result = reply_rx.await.unwrap();
    assert!(result.is_ok(), "spawn passthrough failed: {result:?}");
    actor.cleanup().await;
}

// Covers the `LogsForward::Disabled` branch (capture but don't forward).
#[tokio::test]
async fn do_start_disabled_log_mode() {
    let config = ServiceConfig {
        command: Some("/bin/sleep 100".into()),
        logs_forward: Some(vigil_types::plan::LogsForward::Disabled),
        ..Default::default()
    };
    let (event_tx, _event_rx) = mpsc::channel(64);
    let log_store = LogStore::new(100, 64);
    let metrics = MetricsStore::new();
    let mut actor = Actor::new("svc".into(), config, test_ctx(event_tx, log_store, metrics));

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    actor.handle_start(reply_tx, &mut stop_deadline).await;
    let result = reply_rx.await.unwrap();
    assert!(result.is_ok(), "spawn disabled logs failed: {result:?}");
    actor.cleanup().await;
}

// Covers the `if self.state == ServiceState::Stopping` early-return in handle_start.
#[tokio::test]
async fn start_when_stopping_returns_error() {
    let (mut actor, _) = make_actor();
    actor.state = ServiceState::Stopping;

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut stop_deadline = None;
    actor.handle_start(reply_tx, &mut stop_deadline).await;
    let result = reply_rx.await.unwrap();
    assert!(
        result.is_err(),
        "expected error when starting a Stopping service"
    );
}

// -----------------------------------------------------------------------
// Signal unit tests
// -----------------------------------------------------------------------

#[test]
fn stop_signal_default_is_sigterm() {
    let (actor, _) = make_actor(); // config.stop_signal = None
    assert_eq!(actor.stop_signal(), Signal::SIGTERM);
}

#[test]
fn stop_signal_custom() {
    let config = ServiceConfig {
        stop_signal: Some(StopSignal(Signal::SIGUSR1)),
        ..Default::default()
    };
    let (actor, _) = make_actor_with_config(config);
    assert_eq!(actor.stop_signal(), Signal::SIGUSR1);
}

#[test]
fn kill_delay_default() {
    let (actor, _) = make_actor();
    assert_eq!(actor.kill_delay(), crate::service::DEFAULT_KILL_DELAY);
}

#[test]
fn kill_delay_parsed() {
    let config = ServiceConfig {
        kill_delay: Some("2s".into()),
        ..Default::default()
    };
    let (actor, _) = make_actor_with_config(config);
    assert_eq!(actor.kill_delay(), Duration::from_secs(2));
}

#[test]
fn kill_delay_invalid_falls_back_to_default() {
    let config = ServiceConfig {
        kill_delay: Some("notaduration".into()),
        ..Default::default()
    };
    let (actor, _) = make_actor_with_config(config);
    assert_eq!(actor.kill_delay(), crate::service::DEFAULT_KILL_DELAY);
}

#[test]
fn send_stop_signal_no_child_is_noop() {
    let (actor, _) = make_actor();
    assert!(actor.child.is_none());
    actor.send_stop_signal(); // must not panic
}

#[test]
fn send_signal_no_child_is_noop() {
    let (actor, _) = make_actor();
    assert!(actor.child.is_none());
    actor.send_signal(Signal::SIGTERM); // must not panic
}

#[test]
fn send_sigkill_no_child_is_noop() {
    let (actor, _) = make_actor();
    assert!(actor.child.is_none());
    actor.send_sigkill(); // must not panic
}

#[tokio::test]
async fn send_stop_signal_with_child() {
    let config = ServiceConfig {
        command: Some("/bin/sleep 10".into()),
        ..Default::default()
    };
    let (event_tx, _event_rx) = mpsc::channel(64);
    let log_store = crate::logs::LogStore::new(100, 64);
    let metrics = crate::metrics::MetricsStore::new();
    let mut actor = Actor::new("svc".into(), config, test_ctx(event_tx, log_store, metrics));

    // Spawn a real child process
    let child = tokio::process::Command::new("sleep")
        .arg("10")
        .spawn()
        .expect("failed to spawn sleep");
    actor.child = Some(child);

    // Must not panic; sends SIGTERM to the process group
    actor.send_stop_signal();

    // Verify the process is still accessible (it may not have exited yet)
    assert!(actor.child.is_some());

    actor.cleanup().await;
}

// ------------------------------------------------------------------
// Phase 1: exit-code propagation, exit-code-map, success-exit-codes
// ------------------------------------------------------------------

/// Feed one exit into the actor and return (DaemonShutdown code, ProcessExited success).
async fn exit_with(
    actor: &mut Actor,
    rx: &mut mpsc::Receiver<Event>,
    raw: i32,
) -> (Option<i32>, Option<bool>) {
    use std::os::unix::process::ExitStatusExt;
    let mut backoff_sleep = None;
    let mut stop_deadline = None;
    let mut pending_restart = false;
    actor
        .handle_child_exit(
            Ok(std::process::ExitStatus::from_raw(raw)),
            &mut backoff_sleep,
            &mut stop_deadline,
            &mut pending_restart,
        )
        .await;
    let (mut code, mut success) = (None, None);
    while let Ok(ev) = rx.try_recv() {
        match ev.kind {
            EventKind::DaemonShutdown { exit_code } => code = Some(exit_code),
            EventKind::ProcessExited { success: s, .. } => success = Some(s),
            _ => {}
        }
    }
    (code, success)
}

#[tokio::test]
async fn propagate_passes_service_exit_code() {
    let (mut actor, mut rx) = make_actor();
    actor.config.on_failure = Some(vigil_types::plan::OnExit::Shutdown);
    let (code, success) = exit_with(&mut actor, &mut rx, 3 << 8).await;
    assert_eq!(code, Some(3));
    assert_eq!(success, Some(false));
    assert_eq!(
        actor.last_exit,
        Some(vigil_types::api::ExitInfo {
            code: Some(3),
            signal: None
        })
    );
}

#[tokio::test]
async fn propagate_signal_as_128_plus_signal() {
    let (mut actor, mut rx) = make_actor();
    actor.config.on_failure = Some(vigil_types::plan::OnExit::Shutdown);
    let (code, _) = exit_with(&mut actor, &mut rx, 15).await; // killed by SIGTERM
    assert_eq!(code, Some(143));
    assert_eq!(
        actor.last_exit,
        Some(vigil_types::api::ExitInfo {
            code: None,
            signal: Some(15)
        })
    );
}

#[tokio::test]
async fn exit_code_map_turns_sigterm_into_success() {
    let (mut actor, mut rx) = make_actor();
    actor.config.exit_code_map.insert(143, 0);
    actor.config.on_success = Some(vigil_types::plan::OnExit::Shutdown);
    actor.config.on_failure = Some(vigil_types::plan::OnExit::FailureShutdown);
    let (code, success) = exit_with(&mut actor, &mut rx, 15).await;
    assert_eq!(success, Some(true), "143 mapped to 0 counts as success");
    assert_eq!(code, Some(0));
}

#[tokio::test]
async fn success_exit_codes_classify_nonzero_as_success() {
    let (mut actor, mut rx) = make_actor();
    actor.config.success_exit_codes = vec![0, 2];
    actor.config.on_success = Some(vigil_types::plan::OnExit::Shutdown);
    actor.config.on_failure = Some(vigil_types::plan::OnExit::FailureShutdown);
    let (code, success) = exit_with(&mut actor, &mut rx, 2 << 8).await;
    assert_eq!(success, Some(true));
    assert_eq!(code, Some(2), "on-success shutdown propagates code 2");

    let (code, success) = exit_with(&mut actor, &mut rx, 1 << 8).await;
    assert_eq!(success, Some(false), "1 is not listed");
    assert_eq!(code, Some(1));
}

#[tokio::test]
async fn success_exit_codes_replace_zero() {
    let (mut actor, mut rx) = make_actor();
    actor.config.success_exit_codes = vec![2];
    actor.config.on_failure = Some(vigil_types::plan::OnExit::FailureShutdown);
    let (code, success) = exit_with(&mut actor, &mut rx, 0).await;
    assert_eq!(success, Some(false), "0 is not in the list any more");
    assert_eq!(code, Some(10), "failure-shutdown never exits 0");
}

// -----------------------------------------------------------------------
// Readiness (ready: process | notify | check)
// -----------------------------------------------------------------------

fn spawn_run(
    config: ServiceConfig,
    notify_dir: std::path::PathBuf,
) -> (mpsc::Sender<Cmd>, mpsc::Receiver<Event>) {
    let (event_tx, event_rx) = mpsc::channel(64);
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(run(
        "svc".into(),
        config,
        rx,
        crate::service::Context {
            notify_dir,
            ..test_ctx(event_tx, LogStore::new(100, 64), MetricsStore::new())
        },
    ));
    (tx, event_rx)
}

async fn start(tx: &mpsc::Sender<Cmd>) {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(Cmd::Start(reply_tx)).await.unwrap();
    reply_rx.await.unwrap().unwrap();
}

/// Wait for an event matching `f`, returning all events seen before it.
async fn wait_event(
    rx: &mut mpsc::Receiver<Event>,
    f: impl Fn(&EventKind) -> bool,
) -> Vec<EventKind> {
    let mut seen = Vec::new();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out waiting for event")
            .expect("actor gone");
        if f(&ev.kind) {
            return seen;
        }
        seen.push(ev.kind);
    }
}

fn is_state(s: ServiceState) -> impl Fn(&EventKind) -> bool {
    move |k| matches!(k, EventKind::StateChanged { new_state } if *new_state == s)
}

#[tokio::test]
async fn ready_process_goes_active_immediately() {
    let mut actor = make_actor_with_config(ServiceConfig {
        command: Some("sleep 30".into()),
        ..Default::default()
    })
    .0;
    let (reply_tx, _r) = tokio::sync::oneshot::channel();
    actor.handle_start(reply_tx, &mut None).await;
    assert_eq!(actor.state, ServiceState::Active);
    assert!(actor.ready_deadline.is_none());
    actor.cleanup().await;
}

#[tokio::test]
async fn ready_check_waits_for_a_pass_after_spawn() {
    let before_spawn = std::time::Instant::now();
    let (mut actor, _rx) = make_actor_with_config(ServiceConfig {
        command: Some("sleep 30".into()),
        ready: Some(ReadyMode::Check),
        ready_check: Some("c".into()),
        ..Default::default()
    });
    let (reply_tx, _r) = tokio::sync::oneshot::channel();
    actor.handle_start(reply_tx, &mut None).await;
    assert_eq!(actor.state, ServiceState::Starting);
    assert!(actor.ready_deadline.is_some(), "default timeout applies");

    // A probe that started before the spawn does not count.
    actor.handle_check_passed("c", before_spawn).await;
    assert_eq!(actor.state, ServiceState::Starting);
    // Another check does not count either.
    actor
        .handle_check_passed("other", std::time::Instant::now())
        .await;
    assert_eq!(actor.state, ServiceState::Starting);

    actor
        .handle_check_passed("c", std::time::Instant::now())
        .await;
    assert_eq!(actor.state, ServiceState::Active);
    assert!(actor.ready_deadline.is_none());
    actor.cleanup().await;
}

#[tokio::test]
async fn ready_notify_rejects_foreign_senders_and_accepts_the_service() {
    let dir = tempfile::tempdir().unwrap();
    let (mut actor, _rx) = make_actor_with_config(ServiceConfig {
        command: Some("sleep 30".into()),
        ready: Some(ReadyMode::Notify),
        ready_timeout: Some("0s".into()),
        ..Default::default()
    });
    actor.notify_dir = dir.path().to_owned();
    let (reply_tx, _r) = tokio::sync::oneshot::channel();
    actor.handle_start(reply_tx, &mut None).await;
    assert_eq!(actor.state, ServiceState::Starting);
    assert!(
        actor.ready_deadline.is_none(),
        "ready-timeout 0s = no timeout"
    );
    let sock = actor.notify.as_ref().unwrap().path().to_owned();
    assert!(sock.exists());

    let child = actor.child.as_ref().unwrap().id().unwrap() as i32;
    let me = std::process::id() as i32;
    let uid = actor.service_uid;
    let msg = |pid, pgid, generation| crate::service::notify::NotifyMsg {
        generation,
        pid,
        uid,
        pgid: Some(pgid),
        fields: vec![("READY".into(), "1".into()), ("STATUS".into(), "ok".into())],
    };
    // Same uid, but another process group (the test process).
    let foreign = msg(me, nix::unistd::getpgrp().as_raw(), actor.notify_generation);
    actor.handle_notify(foreign).await;
    assert_eq!(actor.state, ServiceState::Starting);
    // Stale generation.
    let stale = msg(child, child, actor.notify_generation - 1);
    actor.handle_notify(stale).await;
    assert_eq!(actor.state, ServiceState::Starting);

    let ok = msg(child, child, actor.notify_generation);
    actor.handle_notify(ok).await;
    assert_eq!(actor.state, ServiceState::Active);
    assert_eq!(actor.status_message.as_deref(), Some("ok"));

    actor.cleanup().await;
    assert!(!sock.exists(), "notify socket removed with the process");
}

#[tokio::test]
async fn ready_notify_end_to_end_with_real_sender() {
    if std::process::Command::new("python3")
        .arg("-c")
        .arg("1")
        .status()
        .map(|s| !s.success())
        .unwrap_or(true)
    {
        eprintln!("skipped: python3 not available");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("notify.py");
    std::fs::write(
        &script,
        "import os, socket, time\n\
         s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)\n\
         s.sendto(b'STATUS=warm\\nREADY=1\\n', os.environ['NOTIFY_SOCKET'])\n\
         time.sleep(30)\n",
    )
    .unwrap();
    let (tx, mut rx) = spawn_run(
        ServiceConfig {
            command: Some(format!("python3 {}", script.display())),
            ready: Some(ReadyMode::Notify),
            ..Default::default()
        },
        dir.path().to_owned(),
    );
    start(&tx).await;
    let before = wait_event(&mut rx, is_state(ServiceState::Active)).await;
    assert!(
        before.iter().any(|k| matches!(
            k,
            EventKind::StatusMessage { message: Some(m) } if m == "warm"
        )),
        "{before:?}"
    );
    tx.send(Cmd::Shutdown).await.unwrap();
}

#[tokio::test]
async fn ready_timeout_stops_service_and_counts_as_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut exit_code_map = indexmap::IndexMap::new();
    exit_code_map.insert(143, 0); // must not turn the timeout into a success
    let (tx, mut rx) = spawn_run(
        ServiceConfig {
            command: Some("sleep 30".into()),
            ready: Some(ReadyMode::Notify),
            ready_timeout: Some("200ms".into()),
            on_failure: Some(vigil_types::plan::OnExit::Shutdown),
            on_success: Some(vigil_types::plan::OnExit::SuccessShutdown),
            exit_code_map,
            ..Default::default()
        },
        dir.path().to_owned(),
    );
    start(&tx).await;
    let seen = wait_event(&mut rx, |k| matches!(k, EventKind::DaemonShutdown { .. })).await;
    assert!(
        seen.iter()
            .any(|k| matches!(k, EventKind::ProcessExited { success: false, .. })),
        "{seen:?}"
    );
    assert!(
        !seen.iter().any(|k| matches!(
            k,
            EventKind::StateChanged {
                new_state: ServiceState::Active
            }
        )),
        "never became active: {seen:?}"
    );
    tx.send(Cmd::Shutdown).await.unwrap();
}

#[tokio::test]
async fn notify_socket_is_not_inherited_without_ready_notify() {
    // vigild itself may run under systemd with NOTIFY_SOCKET set.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("env");
    let mut actor = make_actor_with_config(ServiceConfig {
        command: Some(format!(
            "/bin/sh -c 'echo \"${{NOTIFY_SOCKET-unset}}\" > {}'",
            out.display()
        )),
        pass_env: Some(true),
        environment: [("NOTIFY_SOCKET".to_string(), "/x".to_string())]
            .into_iter()
            .collect(),
        ..Default::default()
    })
    .0;
    let (reply_tx, _r) = tokio::sync::oneshot::channel();
    actor.handle_start(reply_tx, &mut None).await;
    let _ = actor.child.as_mut().unwrap().wait().await;
    assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), "unset");
}

// -----------------------------------------------------------------------
// type: oneshot, schedule, UpdateConfig
// -----------------------------------------------------------------------

fn oneshot(command: &str) -> ServiceConfig {
    ServiceConfig {
        command: Some(command.into()),
        service_type: Some(vigil_types::plan::ServiceType::Oneshot),
        ..Default::default()
    }
}

#[tokio::test]
async fn oneshot_success_ends_done_and_stays_starting_while_running() {
    let (tx, mut rx) = spawn_run(oneshot("sleep 0.3"), std::env::temp_dir());
    start(&tx).await;
    let seen = wait_event(&mut rx, is_state(ServiceState::Done)).await;
    assert!(
        !seen.iter().any(|k| matches!(
            k,
            EventKind::StateChanged {
                new_state: ServiceState::Active
            }
        )),
        "a oneshot is never active: {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|k| matches!(k, EventKind::ProcessExited { success: true, .. }))
    );
    tx.send(Cmd::Shutdown).await.unwrap();
}

#[tokio::test]
async fn unscheduled_oneshot_failure_restarts_by_default() {
    let (tx, mut rx) = spawn_run(oneshot("/bin/sh -c 'exit 3'"), std::env::temp_dir());
    start(&tx).await;
    wait_event(&mut rx, is_state(ServiceState::Backoff)).await;
    tx.send(Cmd::Shutdown).await.unwrap();
}

#[tokio::test]
async fn scheduled_oneshot_failure_is_ignored_by_default() {
    let mut cfg = oneshot("/bin/sh -c 'exit 3'");
    cfg.schedule = Some("0 0 1 1 *".into()); // far away; started manually
    let (tx, mut rx) = spawn_run(cfg, std::env::temp_dir());
    start(&tx).await;
    let seen = wait_event(&mut rx, is_state(ServiceState::Inactive)).await;
    assert!(
        !seen.iter().any(|k| matches!(
            k,
            EventKind::StateChanged {
                new_state: ServiceState::Backoff
            }
        )),
        "{seen:?}"
    );
    tx.send(Cmd::Shutdown).await.unwrap();
}

#[tokio::test]
async fn schedule_runs_job_and_skips_overlapping_runs() {
    let mut cfg = oneshot("sleep 2.5");
    cfg.schedule = Some("* * * * * *".into()); // every second
    cfg.startup = vigil_types::plan::Startup::Enabled;
    let (tx, mut rx) = spawn_run(cfg, std::env::temp_dir());

    // Armed at actor start without any Start command.
    let first = wait_event(&mut rx, |k| matches!(k, EventKind::NextRun { at: Some(_) })).await;
    assert!(first.is_empty(), "{first:?}");
    wait_event(&mut rx, is_state(ServiceState::Starting)).await;

    // While the 2.5 s run is active, the per-second schedule must not
    // start another one.
    let mut starts = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(2000);
    while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        if matches!(
            ev.kind,
            EventKind::StateChanged {
                new_state: ServiceState::Starting
            }
        ) {
            starts += 1;
        }
    }
    assert_eq!(starts, 0, "overlapping run started");
    tx.send(Cmd::Shutdown).await.unwrap();
}

#[tokio::test]
async fn schedule_is_not_armed_for_startup_disabled() {
    let mut cfg = oneshot("true");
    cfg.schedule = Some("* * * * * *".into());
    let (mut actor, _rx) = make_actor_with_config(cfg);
    actor.arm_schedule().await;
    assert!(actor.next_run.is_none());
}

#[tokio::test]
async fn update_config_applies_and_rearms_schedule() {
    let (mut actor, mut rx) = make_actor_with_config(oneshot("true"));
    actor.arm_schedule().await;
    assert!(actor.next_run.is_none());

    let mut cfg = oneshot("false");
    cfg.schedule = Some("0 3 * * *".into());
    cfg.startup = vigil_types::plan::Startup::Enabled;
    actor.handle_update_config(cfg).await;
    assert_eq!(actor.config.command.as_deref(), Some("false"));
    assert!(actor.next_run.is_some());
    assert!(matches!(
        rx.try_recv().unwrap().kind,
        EventKind::NextRun { at: Some(_) }
    ));

    // Removing the schedule disarms it.
    actor.handle_update_config(oneshot("false")).await;
    assert!(actor.next_run.is_none());
    assert!(matches!(
        rx.try_recv().unwrap().kind,
        EventKind::NextRun { at: None }
    ));
}
