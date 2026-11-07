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
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn spawn_actor(
    config: CheckConfig,
    initial_status: CheckStatus,
) -> (mpsc::Sender<Cmd>, mpsc::Receiver<CheckEvent>) {
    spawn_actor_with_metrics(config, initial_status, MetricsStore::new())
}

fn spawn_actor_with_metrics(
    config: CheckConfig,
    initial_status: CheckStatus,
    metrics: Arc<MetricsStore>,
) -> (mpsc::Sender<Cmd>, mpsc::Receiver<CheckEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(16);
    tokio::spawn(run(
        "test-check".to_string(),
        Setup {
            http: config
                .http
                .as_ref()
                .map(|h| crate::check::probe::http::client(h).unwrap()),
            config,
        },
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
        startup: Some(Startup::Enabled),
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
        Setup { config, http: None },
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

// ------------------------------------------------------------------
// SetHttpClient: a replan hands over a new client
// ------------------------------------------------------------------

async fn spawn_ok_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = axum::Router::new().route("/", axum::routing::get(|| async { "ok" }));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://127.0.0.1:{port}/")
}

fn http_cfg(url: String, delay: &str) -> CheckConfig {
    CheckConfig {
        delay: Some(delay.to_string()),
        period: Some("30ms".to_string()),
        timeout: Some("2s".to_string()),
        threshold: Some(100),
        http: Some(vigil_types::plan::HttpCheck {
            url,
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Spawn an HTTP check actor that starts without a client (every probe
/// fails until SetHttpClient arrives).
fn spawn_without_client(config: CheckConfig) -> (mpsc::Sender<Cmd>, mpsc::Receiver<CheckEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(16);
    tokio::spawn(run(
        "test-check".to_string(),
        Setup { config, http: None },
        Arc::new(IndexMap::new()),
        cmd_rx,
        event_tx,
        MetricsStore::new(),
        CheckStatus::Up,
    ));
    (cmd_tx, event_rx)
}

async fn next_event(rx: &mut mpsc::Receiver<CheckEvent>) -> CheckEvent {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("no check event")
        .expect("event channel closed")
}

#[tokio::test]
async fn set_http_client_in_the_main_loop_keeps_the_failure_count_until_a_pass() {
    let config = http_cfg(spawn_ok_server().await, "10ms");
    let client = crate::check::probe::http::client(config.http.as_ref().unwrap()).unwrap();
    let (tx, mut rx) = spawn_without_client(config);

    // First probe without a client fails and reports the initial status.
    let ev = next_event(&mut rx).await;
    assert!(ev.passed_at.is_none());
    assert!(
        get_status(&tx).await.failures >= 1,
        "the failure count is kept"
    );

    tx.send(Cmd::SetHttpClient(client)).await.unwrap();
    let ev = next_event(&mut rx).await;
    assert_eq!(ev.status, CheckStatus::Up);
    assert!(ev.passed_at.is_some(), "the new client is used");
    assert_eq!(get_status(&tx).await.failures, 0);
    let _ = tx.send(Cmd::Shutdown).await;
}

#[tokio::test]
async fn set_http_client_during_the_delay_is_used_by_the_first_probe() {
    let config = http_cfg(spawn_ok_server().await, "200ms");
    let client = crate::check::probe::http::client(config.http.as_ref().unwrap()).unwrap();
    let (tx, mut rx) = spawn_without_client(config);

    tx.send(Cmd::SetHttpClient(client)).await.unwrap();
    let ev = next_event(&mut rx).await;
    assert!(
        ev.passed_at.is_some(),
        "the first probe uses the new client"
    );
    let _ = tx.send(Cmd::Shutdown).await;
}

// A failed run's reason is in the Down event and in `last-error`; the next
// pass clears it.
#[tokio::test]
async fn failure_reason_is_reported_and_cleared_by_a_pass() {
    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready");
    let cmd = format!(
        "test -e {} || {{ echo database not ready; exit 1; }}",
        ready.display()
    );
    let (tx, mut event_rx) = spawn_actor(exec_cfg(&cmd, 1), CheckStatus::Up);

    let down = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
        .await
        .expect("timed out")
        .unwrap();
    assert_eq!(down.status, CheckStatus::Down);
    assert_eq!(
        down.error.as_deref(),
        Some("exit status 1: database not ready")
    );
    let info = get_status(&tx).await;
    assert_eq!(
        info.last_error.as_deref(),
        Some("exit status 1: database not ready")
    );

    std::fs::write(&ready, "").unwrap();
    let up = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
        .await
        .expect("timed out")
        .unwrap();
    assert_eq!(up.status, CheckStatus::Up);
    assert_eq!(up.error, None);
    assert_eq!(get_status(&tx).await.last_error, None);
    let _ = tx.send(Cmd::Shutdown).await;
}

async fn refresh(tx: &mpsc::Sender<Cmd>) -> vigil_types::api::CheckRefresh {
    let (reply_tx, reply_rx) = oneshot::channel();
    tx.send(Cmd::Refresh(reply_tx)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), reply_rx)
        .await
        .expect("refresh timed out")
        .unwrap()
}

// A refresh during the initial delay runs at once and counts like a
// scheduled run: failures, threshold, Down, an event with the reason.
#[tokio::test]
async fn refresh_runs_now_and_counts_like_a_scheduled_run() {
    let config = CheckConfig {
        delay: Some("1h".to_string()),
        threshold: Some(2),
        exec: Some(ExecCheck {
            command: "echo not ready; exit 1".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let (tx, mut event_rx) = spawn_actor(config, CheckStatus::Up);

    let first = refresh(&tx).await;
    assert_eq!(first.error.as_deref(), Some("exit status 1: not ready"));
    assert_eq!(first.info.status, CheckStatus::Up);
    assert_eq!(first.info.failures, 1);
    // The first run reports the initial status.
    let ev = event_rx.recv().await.unwrap();
    assert_eq!(ev.status, CheckStatus::Up);

    let second = refresh(&tx).await;
    assert_eq!(second.info.status, CheckStatus::Down);
    assert_eq!(second.info.failures, 2);
    assert_eq!(
        second.info.last_error.as_deref(),
        Some("exit status 1: not ready")
    );
    let ev = event_rx.recv().await.unwrap();
    assert_eq!(ev.status, CheckStatus::Down);
    assert_eq!(ev.error.as_deref(), Some("exit status 1: not ready"));

    // GetStatus sees the same state.
    assert_eq!(get_status(&tx).await.failures, 2);
    let _ = tx.send(Cmd::Shutdown).await;
}

// A passing refresh after the delay resets the failures and has no error.
#[tokio::test]
async fn refresh_that_passes_has_no_error() {
    let (tx, _event_rx) = spawn_actor(exec_cfg("true", 3), CheckStatus::Up);
    let result = refresh(&tx).await;
    assert_eq!(result.error, None);
    assert_eq!(result.info.status, CheckStatus::Up);
    assert_eq!(result.info.failures, 0);
    let _ = tx.send(Cmd::Shutdown).await;
}

// A check without an actor (startup: disabled) runs once, detached: the
// result shows only that run.
#[tokio::test]
async fn detached_run_reports_only_this_run() {
    let config = exec_cfg("echo nope >&2; exit 4", 3);
    let result =
        crate::check::run_once_detached("off".into(), config, None, Arc::new(IndexMap::new()))
            .await;
    assert_eq!(result.error.as_deref(), Some("exit status 4: nope"));
    assert_eq!(result.info.status, CheckStatus::Down);
    assert_eq!(result.info.failures, 1);
    assert_eq!(result.info.threshold, 3);

    let ok = crate::check::run_once_detached(
        "off".into(),
        exec_cfg("true", 3),
        None,
        Arc::new(IndexMap::new()),
    )
    .await;
    assert_eq!(ok.error, None);
    assert_eq!(ok.info.status, CheckStatus::Up);
    assert_eq!(ok.info.failures, 0);
}

/// The value of the metric line starting with `prefix` (labels included).
fn metric_value(metrics: &MetricsStore, prefix: &str) -> Option<f64> {
    metrics
        .render()
        .lines()
        .find_map(|l| l.strip_prefix(prefix)?.trim().parse().ok())
}

// vigil_check_up exists from the start; an outage counts once in
// vigil_check_down_count however many runs fail, and its length goes into
// vigil_check_down_seconds when the check recovers.
#[tokio::test]
async fn outage_counts_once_and_its_time_is_added_on_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready");
    let config = CheckConfig {
        level: Some(vigil_types::plan::CheckLevel::Ready),
        delay: Some("1h".to_string()),
        threshold: Some(1),
        exec: Some(ExecCheck {
            command: format!("test -e {}", ready.display()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let metrics = MetricsStore::new();
    metrics.set_checks(&[("test-check", vigil_types::plan::CheckLevel::Ready)]);
    let (tx, _event_rx) = spawn_actor_with_metrics(config, CheckStatus::Up, Arc::clone(&metrics));
    let up = r#"vigil_check_up{check="test-check",level="ready"}"#;
    let count = r#"vigil_check_down_count_total{check="test-check",level="ready"}"#;
    let seconds = r#"vigil_check_down_seconds_total{check="test-check",level="ready"}"#;
    assert_eq!(get_status(&tx).await.status, CheckStatus::Up);
    assert_eq!(metric_value(&metrics, up), Some(1.0));

    refresh(&tx).await;
    refresh(&tx).await;
    assert_eq!(metric_value(&metrics, up), Some(0.0));
    assert_eq!(metric_value(&metrics, count), Some(1.0));

    tokio::time::sleep(Duration::from_millis(200)).await;
    std::fs::write(&ready, "").unwrap();
    assert_eq!(refresh(&tx).await.info.status, CheckStatus::Up);
    assert_eq!(metric_value(&metrics, up), Some(1.0));
    assert_eq!(metric_value(&metrics, count), Some(1.0));
    let down = metric_value(&metrics, seconds).unwrap();
    assert!((0.2..5.0).contains(&down), "down seconds: {down}");
    let _ = tx.send(Cmd::Shutdown).await;
}

// A probe that runs out of time counts with the check's timeout in
// vigil_check_duration_seconds (result="error"), not with the time the
// kill and the output drain took on top.
#[tokio::test]
async fn timed_out_run_counts_the_timeout() {
    let config = CheckConfig {
        delay: Some("1h".to_string()),
        timeout: Some("200ms".to_string()),
        exec: Some(ExecCheck {
            command: "sleep 5".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let metrics = MetricsStore::new();
    metrics.set_checks(&[("test-check", vigil_types::plan::CheckLevel::Alive)]);
    let (tx, _event_rx) = spawn_actor_with_metrics(config, CheckStatus::Up, Arc::clone(&metrics));
    let run = refresh(&tx).await;
    assert_eq!(run.error.as_deref(), Some("timed out after 200ms"));
    let labels = r#"{check="test-check",level="alive",result="error"}"#;
    assert_eq!(
        metric_value(
            &metrics,
            &format!("vigil_check_duration_seconds_sum{labels}")
        ),
        Some(0.2)
    );
    assert_eq!(
        metric_value(
            &metrics,
            &format!("vigil_check_duration_seconds_count{labels}")
        ),
        Some(1.0)
    );
    let _ = tx.send(Cmd::Shutdown).await;
}
