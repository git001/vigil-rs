// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use tokio::sync::{mpsc, oneshot};
use vigil_types::api::{CheckInfo, CheckStatus};
use vigil_types::plan::{CheckConfig, ExecCheck, Startup};

use crate::check::{CheckEvent, Cmd};
use crate::metrics::MetricsStore;

use super::*;

fn exec_cfg(cmd: &str, threshold: u32) -> CheckConfig {
    CheckConfig {
        delay: Some("10ms".to_string()),
        period: Some("30ms".to_string()),
        timeout: Some("5s".to_string()),
        threshold: Some(threshold),
        exec: Some(ExecCheck {
            command: cmd.to_string(),
            service_context: None,
            environment: IndexMap::new(),
            user: None,
            user_id: None,
            group: None,
            group_id: None,
            working_dir: None,
        }),
        ..Default::default()
    }
}

fn spawn_actor(
    config: CheckConfig,
    initial_status: CheckStatus,
) -> (mpsc::Sender<Cmd>, mpsc::Receiver<CheckEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(16);
    let metrics = MetricsStore::new();
    tokio::spawn(run(
        "test-check".to_string(),
        config,
        Arc::new(IndexMap::new()),
        cmd_rx,
        event_tx,
        metrics,
        initial_status,
    ));
    (cmd_tx, event_rx)
}

async fn get_status(tx: &mpsc::Sender<Cmd>) -> CheckInfo {
    let (reply_tx, reply_rx) = oneshot::channel();
    tx.send(Cmd::GetStatus(reply_tx)).await.unwrap();
    reply_rx.await.unwrap()
}

// During the initial delay the actor responds to GetStatus with Up / 0 failures.
#[tokio::test]
async fn get_status_during_delay_returns_up_zero_failures() {
    let config = CheckConfig {
        delay: Some("5s".to_string()),
        exec: Some(ExecCheck {
            command: "false".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let (tx, _) = spawn_actor(config, CheckStatus::Up);
    let info = get_status(&tx).await;
    assert_eq!(info.status, CheckStatus::Up);
    assert_eq!(info.failures, 0);
    assert_eq!(info.name, "test-check");
    assert!(
        info.next_run_in_secs.is_none(),
        "delay phase should not report next_run_in_secs"
    );
    let _ = tx.send(Cmd::Shutdown).await;
}

// Shutdown during the initial delay exits cleanly.
#[tokio::test]
async fn shutdown_during_delay_exits_cleanly() {
    let config = CheckConfig {
        delay: Some("5s".to_string()),
        startup: Startup::Enabled,
        exec: Some(ExecCheck {
            command: "true".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    let (event_tx, _) = mpsc::channel(16);
    let metrics = MetricsStore::new();
    let join = tokio::spawn(run(
        "test-check".to_string(),
        config,
        Arc::new(IndexMap::new()),
        cmd_rx,
        event_tx,
        metrics,
        CheckStatus::Up,
    ));
    cmd_tx.send(Cmd::Shutdown).await.unwrap();
    tokio::time::timeout(Duration::from_millis(500), join)
        .await
        .expect("actor did not exit after Shutdown during delay")
        .unwrap();
}

// On the first check run a CheckEvent is always emitted (first_run flag).
#[tokio::test]
async fn first_run_always_sends_initial_event() {
    let (tx, mut event_rx) = spawn_actor(exec_cfg("true", 3), CheckStatus::Up);
    let ev = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
        .await
        .expect("timed out")
        .unwrap();
    assert_eq!(ev.check, "test-check");
    assert_eq!(ev.status, CheckStatus::Up);
    let _ = tx.send(Cmd::Shutdown).await;
}

// A single failure below the threshold keeps the check Up.
#[tokio::test]
async fn failure_below_threshold_stays_up() {
    // threshold=2: after first failure the check is still Up
    let (tx, mut event_rx) = spawn_actor(exec_cfg("false", 2), CheckStatus::Up);
    // First event is the first_run event (failures=1 < threshold=2 → still Up)
    let ev = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
        .await
        .expect("timed out")
        .unwrap();
    assert_eq!(ev.status, CheckStatus::Up);
    let info = get_status(&tx).await;
    assert_eq!(info.failures, 1);
    assert_eq!(info.status, CheckStatus::Up);
    let _ = tx.send(Cmd::Shutdown).await;
}

// Once failures reach the threshold a Down event is emitted.
#[tokio::test]
async fn failure_reaches_threshold_sends_down_event() {
    let (tx, mut event_rx) = spawn_actor(exec_cfg("false", 2), CheckStatus::Up);
    // Skip first-run event (still Up after failure 1)
    let _ = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
        .await
        .expect("timed out")
        .unwrap();
    // Second event: failures=2 → Down
    let ev = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
        .await
        .expect("timed out")
        .unwrap();
    assert_eq!(ev.status, CheckStatus::Down);
    let _ = tx.send(Cmd::Shutdown).await;
}

// Starting in Down + successful probe → recovery Up event.
#[tokio::test]
async fn recovery_from_initial_down_sends_up_event() {
    let (tx, mut event_rx) = spawn_actor(exec_cfg("true", 1), CheckStatus::Down);
    let ev = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
        .await
        .expect("timed out")
        .unwrap();
    assert_eq!(ev.status, CheckStatus::Up);
    let _ = tx.send(Cmd::Shutdown).await;
}

// GetStatus during the main loop reports current failures/status.
#[tokio::test]
async fn get_status_in_main_loop_reports_current_state() {
    let (tx, mut event_rx) = spawn_actor(exec_cfg("false", 10), CheckStatus::Up);
    // Wait for the first-run event so we know at least one tick has fired.
    let _ = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
        .await
        .expect("timed out")
        .unwrap();
    let info = get_status(&tx).await;
    assert!(info.failures >= 1, "expected at least one failure recorded");
    assert_eq!(info.status, CheckStatus::Up);
    let _ = tx.send(Cmd::Shutdown).await;
}
