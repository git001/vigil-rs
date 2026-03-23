// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use chrono::Utc;
use indexmap::IndexMap;
use tokio::sync::mpsc;
use vigil_types::plan::{Plan, ServiceConfig, Startup};

use crate::logs::LogStore;
use crate::metrics::MetricsStore;
use crate::service::{self, Snapshot};
use crate::state::ServiceState;

use super::{Overlord, ServiceEntry};

// ------------------------------------------------------------------
// Test helpers
// ------------------------------------------------------------------

/// Build a minimal `Overlord` for testing `after_deps_running` and
/// `handle_svc_event`.
///
/// `plan_services`: list of `(name, after, requires, before)` tuples
/// that describe the dependency graph.
///
/// `states`: current state for each service name (services not listed
/// here get `ServiceState::Inactive`).
#[allow(clippy::type_complexity)]
pub(super) fn make_overlord(
    plan_services: &[(&str, &[&str], &[&str], &[&str])],
    states: &[(&str, ServiceState)],
) -> Overlord {
    let mut services_plan: IndexMap<String, ServiceConfig> = IndexMap::new();
    for (name, after, requires, before) in plan_services {
        let cfg = ServiceConfig {
            after: after.iter().map(|s| s.to_string()).collect(),
            requires: requires.iter().map(|s| s.to_string()).collect(),
            before: before.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        services_plan.insert(name.to_string(), cfg);
    }

    let state_map: std::collections::HashMap<&str, ServiceState> = states.iter().copied().collect();

    let mut svc_map: IndexMap<String, ServiceEntry> = IndexMap::new();
    for (name, _, _, _) in plan_services {
        let (tx, _rx) = mpsc::channel(4);
        let handle = service::Handle { tx };
        let state = state_map
            .get(name)
            .copied()
            .unwrap_or(ServiceState::Inactive);
        let snapshot = Snapshot {
            name: name.to_string(),
            state,
            since: Utc::now(),
            startup: Startup::Enabled,
            pid: None,
        };
        svc_map.insert(name.to_string(), ServiceEntry { handle, snapshot });
    }

    let (event_tx, event_rx) = mpsc::channel(8);
    let (check_event_tx, check_event_rx) = mpsc::channel(8);
    let (_, cmd_rx) = mpsc::channel(8);

    Overlord {
        plan: Plan {
            services: services_plan,
            checks: IndexMap::new(),
            alerts: IndexMap::new(),
            layers: Vec::new(),
            alert_queue_depth: None,
            alert_max_queue_time: None,
        },
        services: svc_map,
        checks: IndexMap::new(),
        alert_sender: crate::alert::AlertSender::new(),
        changes: Vec::new(),
        change_counter: 0,
        boot_id: "test".to_string(),
        start_time: Utc::now(),
        http_address: String::new(),
        https_address: None,
        log_store: LogStore::new(64, 64),
        metrics: MetricsStore::new(),
        event_rx,
        event_tx,
        check_event_rx,
        check_event_tx,
        cmd_rx,
        layers_dir: std::path::PathBuf::new(),
        pending_autostart: Vec::new(),
    }
}

// ------------------------------------------------------------------
// after_deps_running — `after:` ordering
// ------------------------------------------------------------------

#[test]
fn after_dep_running_unblocks_service() {
    let ov = make_overlord(
        &[("dep", &[], &[], &[]), ("svc", &["dep"], &[], &[])],
        &[
            ("dep", ServiceState::Active),
            ("svc", ServiceState::Inactive),
        ],
    );
    assert!(ov.after_deps_running("svc"));
}

#[test]
fn after_dep_inactive_blocks_service() {
    let ov = make_overlord(
        &[("dep", &[], &[], &[]), ("svc", &["dep"], &[], &[])],
        &[
            ("dep", ServiceState::Inactive),
            ("svc", ServiceState::Inactive),
        ],
    );
    assert!(!ov.after_deps_running("svc"));
}

#[test]
fn service_with_no_deps_is_always_ready() {
    let ov = make_overlord(&[("svc", &[], &[], &[])], &[]);
    assert!(ov.after_deps_running("svc"));
}

// ------------------------------------------------------------------
// after_deps_running — `before:` (reverse-after) ordering
// ------------------------------------------------------------------

#[test]
fn before_dep_running_unblocks_service() {
    let ov = make_overlord(
        &[("gate", &[], &[], &["svc"]), ("svc", &[], &[], &[])],
        &[
            ("gate", ServiceState::Active),
            ("svc", ServiceState::Inactive),
        ],
    );
    assert!(ov.after_deps_running("svc"));
}

#[test]
fn before_dep_inactive_blocks_service() {
    let ov = make_overlord(
        &[("gate", &[], &[], &["svc"]), ("svc", &[], &[], &[])],
        &[
            ("gate", ServiceState::Inactive),
            ("svc", ServiceState::Inactive),
        ],
    );
    assert!(!ov.after_deps_running("svc"));
}

// ------------------------------------------------------------------
// after_deps_running — `requires:` ordering (identical to `after:`)
// ------------------------------------------------------------------

#[test]
fn requires_dep_running_unblocks_service() {
    let ov = make_overlord(
        &[("db", &[], &[], &[]), ("api", &[], &["db"], &[])],
        &[
            ("db", ServiceState::Active),
            ("api", ServiceState::Inactive),
        ],
    );
    assert!(ov.after_deps_running("api"));
}

#[test]
fn requires_dep_inactive_blocks_service() {
    let ov = make_overlord(
        &[("db", &[], &[], &[]), ("api", &[], &["db"], &[])],
        &[
            ("db", ServiceState::Inactive),
            ("api", ServiceState::Inactive),
        ],
    );
    assert!(!ov.after_deps_running("api"));
}

// ------------------------------------------------------------------
// requires: stop cascade
// ------------------------------------------------------------------

async fn make_requires_overlord() -> (Overlord, mpsc::Receiver<()>) {
    #[allow(clippy::type_complexity)]
    let plan_services: &[(&str, &[&str], &[&str], &[&str])] =
        &[("db", &[], &[], &[]), ("api", &[], &["db"], &[])];
    let states = &[("db", ServiceState::Active), ("api", ServiceState::Active)];
    let mut ov = make_overlord(plan_services, states);

    let (stop_tx, stop_rx) = mpsc::channel::<()>(4);

    for (svc_name, notify) in [("api", Some(stop_tx)), ("db", None)] {
        let (tx, mut rx) = mpsc::channel::<service::Cmd>(8);
        let n = notify;
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    service::Cmd::Stop(reply) => {
                        if let Some(ref t) = n {
                            let _ = t.try_send(());
                        }
                        let _ = reply.send(Ok(()));
                    }
                    service::Cmd::Start(reply) => {
                        let _ = reply.send(Ok(()));
                    }
                    service::Cmd::Restart(reply) => {
                        let _ = reply.send(Ok(()));
                    }
                    _ => {}
                }
            }
        });
        if let Some(entry) = ov.services.get_mut(svc_name) {
            entry.handle = service::Handle { tx };
        }
    }

    (ov, stop_rx)
}

#[tokio::test]
async fn requires_stop_cascade_fires_on_inactive() {
    let (mut ov, mut stop_rx) = make_requires_overlord().await;
    let event = service::Event {
        service: "db".to_string(),
        kind: service::EventKind::StateChanged {
            new_state: ServiceState::Inactive,
        },
    };
    ov.handle_svc_event(event).await;
    assert!(
        stop_rx.try_recv().is_ok(),
        "Stop was never sent to api on Inactive"
    );
}

#[tokio::test]
async fn requires_stop_cascade_fires_on_error() {
    let (mut ov, mut stop_rx) = make_requires_overlord().await;
    let event = service::Event {
        service: "db".to_string(),
        kind: service::EventKind::StateChanged {
            new_state: ServiceState::Error,
        },
    };
    ov.handle_svc_event(event).await;
    assert!(
        stop_rx.try_recv().is_ok(),
        "Stop was never sent to api on Error"
    );
}

#[tokio::test]
async fn requires_stop_cascade_does_not_fire_on_backoff() {
    let (mut ov, mut stop_rx) = make_requires_overlord().await;
    let event = service::Event {
        service: "db".to_string(),
        kind: service::EventKind::StateChanged {
            new_state: ServiceState::Backoff,
        },
    };
    ov.handle_svc_event(event).await;
    assert!(
        stop_rx.try_recv().is_err(),
        "Stop was wrongly sent to api during Backoff"
    );
}

#[tokio::test]
async fn requires_stop_cascade_does_not_fire_on_stopping() {
    let (mut ov, mut stop_rx) = make_requires_overlord().await;
    let event = service::Event {
        service: "db".to_string(),
        kind: service::EventKind::StateChanged {
            new_state: ServiceState::Stopping,
        },
    };
    ov.handle_svc_event(event).await;
    assert!(
        stop_rx.try_recv().is_err(),
        "Stop was wrongly sent to api during Stopping"
    );
}

// ------------------------------------------------------------------
// handle_svc_event — DaemonShutdown and ProcessExited
// ------------------------------------------------------------------

#[tokio::test]
async fn daemon_shutdown_event_returns_exit_code() {
    let mut ov = make_overlord(&[("svc", &[], &[], &[])], &[]);
    let event = service::Event {
        service: "svc".to_string(),
        kind: service::EventKind::DaemonShutdown { exit_code: 42 },
    };
    let result = ov.handle_svc_event(event).await;
    assert_eq!(result, Some(42));
}

#[tokio::test]
async fn process_exited_event_returns_none() {
    let mut ov = make_overlord(&[("svc", &[], &[], &[])], &[]);
    let event = service::Event {
        service: "svc".to_string(),
        kind: service::EventKind::ProcessExited { success: true },
    };
    let result = ov.handle_svc_event(event).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn state_changed_event_returns_none() {
    let mut ov = make_overlord(&[("svc", &[], &[], &[])], &[]);
    let event = service::Event {
        service: "svc".to_string(),
        kind: service::EventKind::StateChanged {
            new_state: ServiceState::Active,
        },
    };
    let result = ov.handle_svc_event(event).await;
    assert!(result.is_none());
}

// ------------------------------------------------------------------
// fmt_on_exit
// ------------------------------------------------------------------

#[test]
fn fmt_on_exit_all_variants() {
    use super::handlers::fmt_on_exit;
    use vigil_types::plan::OnExit;
    assert_eq!(fmt_on_exit(OnExit::Restart), "restart");
    assert_eq!(fmt_on_exit(OnExit::Ignore), "ignore");
    assert_eq!(fmt_on_exit(OnExit::Shutdown), "shutdown");
    assert_eq!(fmt_on_exit(OnExit::FailureShutdown), "failure-shutdown");
    assert_eq!(fmt_on_exit(OnExit::SuccessShutdown), "success-shutdown");
}

// ------------------------------------------------------------------
// stop_all — does not panic on empty overlord
// ------------------------------------------------------------------

#[tokio::test]
async fn stop_all_empty_overlord_does_not_panic() {
    let mut ov = make_overlord(&[], &[]);
    ov.stop_all().await;
}

// ------------------------------------------------------------------
// handle_check_event — on_check_failure actions
// ------------------------------------------------------------------

#[tokio::test]
async fn on_check_failure_ignore_sends_no_command() {
    let mut ov = make_overlord(&[("svc", &[], &[], &[])], &[("svc", ServiceState::Active)]);
    if let Some(cfg) = ov.plan.services.get_mut("svc") {
        cfg.on_check_failure
            .insert("my-check".to_string(), vigil_types::plan::OnExit::Ignore);
    }
    let (tx, mut rx) = mpsc::channel::<service::Cmd>(8);
    if let Some(entry) = ov.services.get_mut("svc") {
        entry.handle = service::Handle { tx };
    }

    let event = crate::check::CheckEvent {
        check: "my-check".to_string(),
        status: vigil_types::api::CheckStatus::Down,
    };
    ov.handle_check_event(event).await;

    assert!(
        rx.try_recv().is_err(),
        "unexpected command sent for Ignore action"
    );
}

#[tokio::test]
async fn on_check_failure_restart_sends_restart_command() {
    let mut ov = make_overlord(&[("svc", &[], &[], &[])], &[("svc", ServiceState::Active)]);
    if let Some(cfg) = ov.plan.services.get_mut("svc") {
        cfg.on_check_failure
            .insert("my-check".to_string(), vigil_types::plan::OnExit::Restart);
    }
    let (tx, mut rx) = mpsc::channel::<service::Cmd>(8);
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            if let service::Cmd::Restart(reply) = cmd {
                let _ = reply.send(Ok(()));
            }
        }
    });
    let (tx2, rx2) = mpsc::channel::<service::Cmd>(8);
    let (obs_tx, mut obs_rx) = mpsc::channel::<()>(4);
    tokio::spawn(async move {
        let mut rx = rx2;
        while let Some(cmd) = rx.recv().await {
            if let service::Cmd::Restart(reply) = cmd {
                let _ = obs_tx.try_send(());
                let _ = reply.send(Ok(()));
            }
        }
    });
    drop(tx);
    if let Some(entry) = ov.services.get_mut("svc") {
        entry.handle = service::Handle { tx: tx2 };
    }

    let event = crate::check::CheckEvent {
        check: "my-check".to_string(),
        status: vigil_types::api::CheckStatus::Down,
    };
    ov.handle_check_event(event).await;

    assert!(
        obs_rx.try_recv().is_ok(),
        "expected Restart command for on-check-failure: restart"
    );
}

#[tokio::test]
async fn check_event_up_does_not_trigger_action() {
    let mut ov = make_overlord(&[("svc", &[], &[], &[])], &[("svc", ServiceState::Active)]);
    if let Some(cfg) = ov.plan.services.get_mut("svc") {
        cfg.on_check_failure
            .insert("my-check".to_string(), vigil_types::plan::OnExit::Restart);
    }
    let (tx, mut rx) = mpsc::channel::<service::Cmd>(8);
    if let Some(entry) = ov.services.get_mut("svc") {
        entry.handle = service::Handle { tx };
    }

    let event = crate::check::CheckEvent {
        check: "my-check".to_string(),
        status: vigil_types::api::CheckStatus::Up,
    };
    ov.handle_check_event(event).await;
    assert!(
        rx.try_recv().is_err(),
        "unexpected command sent for Up event"
    );
}

// ------------------------------------------------------------------
// pending_autostart unblocking via try_start_pending
// ------------------------------------------------------------------

#[tokio::test]
async fn state_changed_to_active_unblocks_pending_autostart() {
    let mut ov = make_overlord(
        &[("dep", &[], &[], &[]), ("svc", &["dep"], &[], &[])],
        &[
            ("dep", ServiceState::Inactive),
            ("svc", ServiceState::Inactive),
        ],
    );
    ov.pending_autostart.push("svc".to_string());

    let (tx, mut rx) = mpsc::channel::<service::Cmd>(8);
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            if let service::Cmd::Start(reply) = cmd {
                let _ = reply.send(Ok(()));
            }
        }
    });
    let (tx2, rx2) = mpsc::channel::<service::Cmd>(8);
    let (started_tx, mut started_rx) = mpsc::channel::<()>(4);
    tokio::spawn(async move {
        let mut rx = rx2;
        while let Some(cmd) = rx.recv().await {
            if let service::Cmd::Start(reply) = cmd {
                let _ = started_tx.try_send(());
                let _ = reply.send(Ok(()));
            }
        }
    });
    drop(tx);
    if let Some(e) = ov.services.get_mut("svc") {
        e.handle = service::Handle { tx: tx2 };
    }

    if let Some(e) = ov.services.get_mut("dep") {
        e.snapshot.state = ServiceState::Active;
    }
    let event = service::Event {
        service: "dep".to_string(),
        kind: service::EventKind::StateChanged {
            new_state: ServiceState::Active,
        },
    };
    ov.handle_svc_event(event).await;

    assert!(
        started_rx.try_recv().is_ok(),
        "svc was not started after dep became Active"
    );
    assert!(
        ov.pending_autostart.is_empty(),
        "svc still in pending_autostart after start"
    );
}

// ------------------------------------------------------------------
// reload_layers — queue limit propagation
// ------------------------------------------------------------------

#[tokio::test]
async fn reload_layers_propagates_queue_limits() {
    use std::sync::atomic::Ordering;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("001-base.yaml"),
        "alerts:\n  max-queue-depth: 128\n  max-queue-time: 10s\n",
    )
    .unwrap();

    let mut ov = make_overlord(&[], &[]);
    ov.alert_sender.spawn_worker();
    ov.layers_dir = dir.path().to_path_buf();

    ov.reload_layers().await.unwrap();

    assert_eq!(ov.alert_sender.queue_depth, 128);
    assert_eq!(ov.alert_sender.max_age_secs.load(Ordering::Relaxed), 10);
}

#[tokio::test]
async fn reload_layers_defaults_when_not_specified() {
    use std::sync::atomic::Ordering;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("001-base.yaml"), "").unwrap();

    let mut ov = make_overlord(&[], &[]);
    ov.alert_sender.spawn_worker();
    ov.layers_dir = dir.path().to_path_buf();

    ov.reload_layers().await.unwrap();

    assert_eq!(
        ov.alert_sender.queue_depth,
        crate::alert::DEFAULT_DELIVERY_QUEUE
    );
    assert_eq!(
        ov.alert_sender.max_age_secs.load(Ordering::Relaxed),
        crate::alert::DEFAULT_DELIVERY_AGE.as_secs(),
    );
}

// ------------------------------------------------------------------
// after_deps_running — unknown service
// ------------------------------------------------------------------

#[test]
fn after_deps_running_unknown_service_returns_true() {
    let ov = make_overlord(&[], &[]);
    // Service not in plan → no deps → always ready
    assert!(ov.after_deps_running("nonexistent"));
}

// ------------------------------------------------------------------
// sync_actors — checks with startup: disabled are not started
// ------------------------------------------------------------------

#[tokio::test]
async fn sync_actors_skips_disabled_check() {
    use tempfile::TempDir;
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("001.yaml"),
        "checks:\n  my-check:\n    startup: disabled\n    exec:\n      command: \"true\"\n",
    )
    .unwrap();
    let mut ov = make_overlord(&[], &[]);
    ov.alert_sender.spawn_worker();
    ov.layers_dir = dir.path().to_path_buf();
    ov.reload_layers().await.unwrap();
    assert!(ov.checks.is_empty(), "disabled check should not be started");
}

// sync_actors — checks with startup: enabled are started
#[tokio::test]
async fn sync_actors_starts_enabled_check() {
    use tempfile::TempDir;
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("001.yaml"),
        "checks:\n  my-check:\n    startup: enabled\n    exec:\n      command: \"true\"\n",
    )
    .unwrap();
    let mut ov = make_overlord(&[], &[]);
    ov.alert_sender.spawn_worker();
    ov.layers_dir = dir.path().to_path_buf();
    ov.reload_layers().await.unwrap();
    assert_eq!(ov.checks.len(), 1);
    assert!(ov.checks.contains_key("my-check"));
}

// ------------------------------------------------------------------
// reload_layers — removed service/check gets Shutdown
// ------------------------------------------------------------------

#[tokio::test]
async fn reload_layers_removes_service_not_in_new_plan() {
    use tempfile::TempDir;
    let dir = TempDir::new().unwrap();
    // First plan: one service
    std::fs::write(
        dir.path().join("001.yaml"),
        "services:\n  old-svc:\n    command: sleep 999\n",
    )
    .unwrap();

    let mut ov = make_overlord(&[], &[]);
    ov.alert_sender.spawn_worker();
    ov.layers_dir = dir.path().to_path_buf();
    ov.reload_layers().await.unwrap();
    assert!(ov.services.contains_key("old-svc"));

    // Second plan: service removed
    std::fs::write(dir.path().join("001.yaml"), "").unwrap();
    ov.reload_layers().await.unwrap();
    assert!(
        !ov.services.contains_key("old-svc"),
        "removed service should no longer be tracked"
    );
}

#[tokio::test]
async fn reload_layers_removes_check_not_in_new_plan() {
    use tempfile::TempDir;
    let dir = TempDir::new().unwrap();
    // First plan: one check
    std::fs::write(
        dir.path().join("001.yaml"),
        "checks:\n  old-check:\n    startup: enabled\n    exec:\n      command: \"true\"\n",
    )
    .unwrap();

    let mut ov = make_overlord(&[], &[]);
    ov.alert_sender.spawn_worker();
    ov.layers_dir = dir.path().to_path_buf();
    ov.reload_layers().await.unwrap();
    assert!(ov.checks.contains_key("old-check"));

    // Second plan: check removed
    std::fs::write(dir.path().join("001.yaml"), "").unwrap();
    ov.reload_layers().await.unwrap();
    assert!(
        !ov.checks.contains_key("old-check"),
        "removed check should no longer be tracked"
    );
}

// ------------------------------------------------------------------
// sync_actors — check restart on config change
// ------------------------------------------------------------------

// Covers the `if old_json != new_json` branch in sync_actors that restarts
// a check actor when its config has changed since the previous plan load.
#[tokio::test]
async fn sync_actors_restarts_check_on_config_change() {
    use tempfile::TempDir;
    let dir = TempDir::new().unwrap();

    // Initial plan: check with command "sleep 60"
    std::fs::write(
        dir.path().join("001.yaml"),
        "checks:\n  my-check:\n    startup: enabled\n    exec:\n      command: \"sleep 60\"\n",
    )
    .unwrap();

    let mut ov = make_overlord(&[], &[]);
    ov.alert_sender.spawn_worker();
    ov.layers_dir = dir.path().to_path_buf();
    ov.reload_layers().await.unwrap();
    assert_eq!(ov.checks.len(), 1);

    // Change the check config to trigger the restart path.
    std::fs::write(
        dir.path().join("001.yaml"),
        "checks:\n  my-check:\n    startup: enabled\n    exec:\n      command: \"sleep 999\"\n",
    )
    .unwrap();

    ov.reload_layers().await.unwrap();
    // Check is still present (restarted with new config).
    assert_eq!(ov.checks.len(), 1);
    assert!(ov.checks.contains_key("my-check"));
}

// ------------------------------------------------------------------
// error state warning for blocked services
// ------------------------------------------------------------------

#[tokio::test]
async fn error_state_warns_blocked_pending_services() {
    // dep enters Error while svc is still pending autostart waiting for dep
    let mut ov = make_overlord(
        &[("dep", &[], &[], &[]), ("svc", &["dep"], &[], &[])],
        &[
            ("dep", ServiceState::Inactive),
            ("svc", ServiceState::Inactive),
        ],
    );
    ov.pending_autostart.push("svc".to_string());

    // Simulate dep entering Error — should warn about "svc" being blocked
    let event = service::Event {
        service: "dep".to_string(),
        kind: service::EventKind::StateChanged {
            new_state: ServiceState::Error,
        },
    };
    // Just verify it does not panic and returns None (no daemon shutdown)
    let result = ov.handle_svc_event(event).await;
    assert!(result.is_none());
    // svc remains in pending_autostart (it will never start, but that's expected)
    assert!(ov.pending_autostart.contains(&"svc".to_string()));
}

#[tokio::test]
async fn sync_actors_restarts_check_on_success_statuses_change() {
    use tempfile::TempDir;
    let dir = TempDir::new().unwrap();

    std::fs::write(
        dir.path().join("001.yaml"),
        "checks:\n  my-check:\n    startup: enabled\n    http:\n      url: http://localhost:8080/healthz\n      success-statuses: [301]\n",
    ).unwrap();

    let mut ov = make_overlord(&[], &[]);
    ov.alert_sender.spawn_worker();
    ov.layers_dir = dir.path().to_path_buf();
    ov.reload_layers().await.unwrap();

    let stored = serde_json::to_string(&ov.checks.get("my-check").unwrap().config).unwrap();
    assert!(
        stored.contains("301"),
        "stored config should have 301, got: {stored}"
    );

    std::fs::write(
        dir.path().join("001.yaml"),
        "checks:\n  my-check:\n    startup: enabled\n    http:\n      url: http://localhost:8080/healthz\n      success-statuses: [303]\n",
    ).unwrap();

    ov.reload_layers().await.unwrap();

    let updated = serde_json::to_string(&ov.checks.get("my-check").unwrap().config).unwrap();
    assert!(
        updated.contains("303"),
        "updated config should have 303, got: {updated}"
    );
}
