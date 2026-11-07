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
            last_exit: None,
            status_message: None,
            next_run: None,
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
            identities: IndexMap::new(),
            layers: Vec::new(),
            alert_queue_depth: None,
            alert_max_queue_time: None,
        },
        services: svc_map,
        checks: IndexMap::new(),
        check_clients: IndexMap::new(),
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
        exit_code_mode: service::ExitCodeMode::default(),
        notify_dir: std::env::temp_dir(),
        healthy: Default::default(),
        identity_store: crate::identity::IdentityStore::new(),
        watched_files: Default::default(),
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
    let mut ov = make_overlord(
        &[("dep", &[], &[], &[]), ("svc", &["dep"], &[], &[])],
        &[
            ("dep", ServiceState::Inactive),
            ("svc", ServiceState::Inactive),
        ],
    );
    // An autostarted dependency that is not ready yet blocks (a disabled
    // one would be ignored, see unmet_deps).
    ov.plan.services.get_mut("dep").unwrap().startup = Some(Startup::Enabled);
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
    let mut ov = make_overlord(
        &[("gate", &[], &[], &["svc"]), ("svc", &[], &[], &[])],
        &[
            ("gate", ServiceState::Inactive),
            ("svc", ServiceState::Inactive),
        ],
    );
    ov.plan.services.get_mut("gate").unwrap().startup = Some(Startup::Enabled);
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
async fn process_exited_event_returns_none_and_records_last_exit() {
    let mut ov = make_overlord(&[("svc", &[], &[], &[])], &[]);
    let exit = vigil_types::api::ExitInfo {
        code: None,
        signal: Some(15),
    };
    let event = service::Event {
        service: "svc".to_string(),
        kind: service::EventKind::ProcessExited {
            success: false,
            exit: Some(exit),
        },
    };
    let result = ov.handle_svc_event(event).await;
    assert!(result.is_none());
    assert_eq!(ov.services["svc"].snapshot.last_exit, Some(exit));
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
        passed_at: None,
        error: None,
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
        passed_at: None,
        error: None,
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
        passed_at: None,
        error: None,
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
    let m = ov.metrics.render();
    assert!(m.contains("vigil_services_count 0"), "{m}");
    assert!(
        !m.contains("vigil_service_info{service=\"old-svc\"}"),
        "{m}"
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

// ------------------------------------------------------------------
// Readiness: dependents wait for Active, check passes are forwarded
// ------------------------------------------------------------------

#[test]
fn after_dep_starting_blocks_service() {
    let ov = make_overlord(
        &[("dep", &[], &[], &[]), ("svc", &["dep"], &["dep"], &[])],
        &[("dep", ServiceState::Starting)],
    );
    assert!(
        !ov.after_deps_running("svc"),
        "a dependency that is not ready yet must block"
    );
}

#[tokio::test]
async fn state_changed_to_starting_does_not_unblock_pending() {
    let mut ov = make_overlord(&[("dep", &[], &[], &[]), ("svc", &["dep"], &[], &[])], &[]);
    ov.pending_autostart.push("svc".to_string());
    if let Some(e) = ov.services.get_mut("dep") {
        e.snapshot.state = ServiceState::Starting;
    }
    ov.handle_svc_event(service::Event {
        service: "dep".to_string(),
        kind: service::EventKind::StateChanged {
            new_state: ServiceState::Starting,
        },
    })
    .await;
    assert_eq!(ov.pending_autostart, vec!["svc".to_string()]);
}

#[tokio::test]
async fn status_message_event_updates_snapshot() {
    let mut ov = make_overlord(&[("svc", &[], &[], &[])], &[]);
    ov.handle_svc_event(service::Event {
        service: "svc".to_string(),
        kind: service::EventKind::StatusMessage {
            message: Some("loading 3/5".into()),
        },
    })
    .await;
    assert_eq!(
        ov.services["svc"].snapshot.status_message.as_deref(),
        Some("loading 3/5")
    );
}

#[tokio::test]
async fn check_pass_is_forwarded_only_to_starting_ready_check_services() {
    let mut ov = make_overlord(
        &[
            ("waiting", &[], &[], &[]),
            ("active", &[], &[], &[]),
            ("other", &[], &[], &[]),
        ],
        &[
            ("waiting", ServiceState::Starting),
            ("active", ServiceState::Active),
            ("other", ServiceState::Starting),
        ],
    );
    for name in ["waiting", "active"] {
        let cfg = ov.plan.services.get_mut(name).unwrap();
        cfg.ready = Some(vigil_types::plan::ReadyMode::Check);
        cfg.ready_check = Some("c".into());
    }
    let mut rxs = std::collections::HashMap::new();
    for name in ["waiting", "active", "other"] {
        let (tx, rx) = mpsc::channel(4);
        ov.services.get_mut(name).unwrap().handle = service::Handle { tx };
        rxs.insert(name, rx);
    }

    let at = std::time::Instant::now();
    ov.handle_check_event(crate::check::CheckEvent {
        check: "c".into(),
        status: vigil_types::api::CheckStatus::Up,
        passed_at: Some(at),
        error: None,
    })
    .await;

    match rxs.get_mut("waiting").unwrap().try_recv() {
        Ok(service::Cmd::CheckPassed { check, started_at }) => {
            assert_eq!(check, "c");
            assert_eq!(started_at, at);
        }
        _ => panic!("waiting service did not get CheckPassed"),
    }
    assert!(rxs.get_mut("active").unwrap().try_recv().is_err());
    assert!(rxs.get_mut("other").unwrap().try_recv().is_err());
}

// ------------------------------------------------------------------
// oneshot: Done unblocks dependents; scheduled jobs are not autostarted;
// replan passes changed config to existing actors
// ------------------------------------------------------------------

#[test]
fn done_oneshot_unblocks_dependents() {
    let ov = make_overlord(
        &[("migrate", &[], &[], &[]), ("app", &["migrate"], &[], &[])],
        &[("migrate", ServiceState::Done)],
    );
    assert!(ov.after_deps_running("app"));
}

#[tokio::test]
async fn autostart_skips_scheduled_jobs() {
    let mut ov = make_overlord(&[("job", &[], &[], &[])], &[]);
    ov.plan.services.get_mut("job").unwrap().startup = Some(Startup::Enabled);
    ov.plan.services.get_mut("job").unwrap().schedule = Some("0 3 * * *".into());
    let (tx, mut rx) = mpsc::channel(4);
    ov.services.get_mut("job").unwrap().handle = service::Handle { tx };
    ov.autostart().await;
    assert!(rx.try_recv().is_err(), "scheduled job must not be started");
    assert!(ov.pending_autostart.is_empty());
}

#[tokio::test]
async fn sync_actors_passes_config_to_existing_actors() {
    let mut ov = make_overlord(&[("svc", &[], &[], &[])], &[]);
    let (tx, mut rx) = mpsc::channel(4);
    ov.services.get_mut("svc").unwrap().handle = service::Handle { tx };
    ov.plan.services.get_mut("svc").unwrap().command = Some("/bin/new".into());
    ov.sync_actors();
    match rx.try_recv() {
        Ok(service::Cmd::UpdateConfig(cfg)) => {
            assert_eq!(cfg.command.as_deref(), Some("/bin/new"));
        }
        _ => panic!("no UpdateConfig sent"),
    }
}

#[tokio::test]
async fn autostart_does_not_rerun_done_oneshot() {
    let mut ov = make_overlord(
        &[("migrate", &[], &[], &[])],
        &[("migrate", ServiceState::Done)],
    );
    ov.plan.services.get_mut("migrate").unwrap().startup = Some(Startup::Enabled);
    let (tx, mut rx) = mpsc::channel(4);
    ov.services.get_mut("migrate").unwrap().handle = service::Handle { tx };
    ov.autostart().await;
    assert!(
        rx.try_recv().is_err(),
        "done oneshot must not run again on replan"
    );
}

// ------------------------------------------------------------------
// Alerts: checks of an unhealthy service stay quiet
// ------------------------------------------------------------------

fn alerts_fired(ov: &Overlord) -> u64 {
    ov.metrics
        .render()
        .lines()
        .find(|l| l.starts_with("vigil_alert_fire_count_total{alert=\"ops\"}"))
        .and_then(|l| l.rsplit(' ').next())
        .map_or(0, |n| n.parse().unwrap())
}

async fn svc_event(ov: &mut Overlord, kind: service::EventKind) {
    ov.handle_svc_event(service::Event {
        service: "app".into(),
        kind,
    })
    .await;
}

async fn check_event(ov: &mut Overlord, status: vigil_types::api::CheckStatus) {
    ov.handle_check_event(crate::check::CheckEvent {
        check: "web".into(),
        status,
        passed_at: None,
        error: None,
    })
    .await;
}

#[tokio::test]
async fn checks_of_an_unhealthy_service_are_held_back() {
    use vigil_types::api::CheckStatus;
    let mut ov = make_overlord(&[("app", &[], &[], &[])], &[("app", ServiceState::Active)]);
    ov.plan
        .services
        .get_mut("app")
        .unwrap()
        .on_check_failure
        .insert("web".into(), vigil_types::plan::OnExit::Ignore);
    ov.plan.checks.insert("web".into(), Default::default());
    let alert = vigil_types::plan::AlertConfig {
        url: "http://127.0.0.1:1/".into(),
        on_check: vec!["web".into()],
        on_service: vec!["*".into()],
        ..Default::default()
    };
    let alerts = [("ops".to_string(), alert)].into_iter().collect();
    ov.alert_sender.update_alerts(
        crate::alert::prepare_alerts(&alerts, &crate::alert::LoadEnv::process()).unwrap(),
    );

    // Not healthy yet (just started): the failing check is held back.
    assert!(ov.check_quiet("web"));
    check_event(&mut ov, CheckStatus::Down).await;
    assert_eq!(alerts_fired(&ov), 0);

    // Healthy: nothing to resolve for the service, the held check fires.
    svc_event(&mut ov, service::EventKind::Healthy).await;
    assert!(!ov.check_quiet("web"));
    assert_eq!(alerts_fired(&ov), 1);

    // The service crashes (fires), the check recovers meanwhile (held).
    svc_event(
        &mut ov,
        service::EventKind::Unhealthy {
            event: "backoff",
            message: "exited with code 3, restarting in 1s".into(),
        },
    )
    .await;
    assert_eq!(alerts_fired(&ov), 2);
    check_event(&mut ov, CheckStatus::Up).await;
    assert_eq!(alerts_fired(&ov), 2);

    // Healthy again: the service alert and the check alert both resolve.
    svc_event(&mut ov, service::EventKind::Healthy).await;
    assert_eq!(alerts_fired(&ov), 4);
}

#[test]
fn a_check_belongs_to_the_service_it_names() {
    let mut ov = make_overlord(&[("app", &[], &[], &[])], &[("app", ServiceState::Active)]);
    let named = vigil_types::plan::CheckConfig {
        service: Some("app".into()),
        ..Default::default()
    };
    ov.plan.checks.insert("metrics".into(), named);
    ov.plan.checks.insert("other".into(), Default::default());
    assert!(ov.check_quiet("metrics"));
    assert!(!ov.check_quiet("other"), "unrelated checks always alert");
    ov.healthy.insert("app".into());
    assert!(!ov.check_quiet("metrics"));
}

// ------------------------------------------------------------------
// reload_layers — invalid layers keep the plan and alert
// ------------------------------------------------------------------

#[tokio::test]
async fn invalid_layers_keep_the_plan_and_alert_until_a_valid_reload() {
    use tempfile::TempDir;
    let dir = TempDir::new().unwrap();
    let layer = dir.path().join("001.yaml");
    std::fs::write(
        &layer,
        "services:\n  app:\n    command: sleep 999\nalerts:\n  ops:\n    url: http://127.0.0.1:1/\n    on-vigild: [layers]\n",
    )
    .unwrap();
    let mut ov = make_overlord(&[], &[]);
    ov.alert_sender.spawn_worker();
    ov.layers_dir = dir.path().to_path_buf();
    ov.reload_layers().await.unwrap();
    assert_eq!(alerts_fired(&ov), 0, "a valid reload resolves nothing");

    std::fs::write(&layer, "services:\n  app:\n    command: [broken\n").unwrap();
    let err = ov.reload_layers().await.unwrap_err();
    assert!(format!("{err:#}").contains("001.yaml"), "{err:#}");
    assert!(ov.plan.services.contains_key("app"), "running plan kept");
    assert!(ov.plan.alerts.contains_key("ops"));
    assert_eq!(alerts_fired(&ov), 1, "layers/invalid");
    assert!(ov.reload_layers().await.is_err());
    assert_eq!(alerts_fired(&ov), 1, "same problem: no new alert");

    std::fs::write(
        &layer,
        "services:\n  app:\n    command: sleep 998\nalerts:\n  ops:\n    url: http://127.0.0.1:1/\n    on-vigild: [layers]\n",
    )
    .unwrap();
    ov.reload_layers().await.unwrap();
    assert_eq!(alerts_fired(&ov), 2, "recovered");

    let m = ov.metrics.render();
    assert!(
        m.contains("vigil_layers_reloads_total{result=\"ok\"} 2"),
        "{m}"
    );
    assert!(
        m.contains("vigil_layers_reloads_total{result=\"invalid\"} 2"),
        "{m}"
    );
    ov.stop_all().await;
}

// ------------------------------------------------------------------
// sync_actors — disabled checks stop, a changed service context restarts
// ------------------------------------------------------------------

#[tokio::test]
async fn replan_to_disabled_stops_a_running_check() {
    use tempfile::TempDir;
    let dir = TempDir::new().unwrap();
    let layer = dir.path().join("001.yaml");
    std::fs::write(
        &layer,
        "checks:\n  c:\n    startup: enabled\n    exec:\n      command: \"true\"\n",
    )
    .unwrap();
    let mut ov = make_overlord(&[], &[]);
    ov.alert_sender.spawn_worker();
    ov.layers_dir = dir.path().to_path_buf();
    ov.reload_layers().await.unwrap();
    assert!(ov.checks.contains_key("c"));

    std::fs::write(
        &layer,
        "checks:\n  c:\n    startup: disabled\n    exec:\n      command: \"true\"\n",
    )
    .unwrap();
    ov.reload_layers().await.unwrap();
    assert!(!ov.checks.contains_key("c"), "disabled check must stop");
}

#[tokio::test]
async fn changed_service_context_restarts_the_exec_check() {
    use tempfile::TempDir;
    let dir = TempDir::new().unwrap();
    let layer = dir.path().join("001.yaml");
    let write = |env: &str| {
        std::fs::write(
            &layer,
            format!(
                "services:\n  app:\n    command: sleep 300\n    environment:\n      DB_HOST: {env}\n\
                 checks:\n  c:\n    startup: enabled\n    exec:\n      command: \"true\"\n      service-context: app\n"
            ),
        )
        .unwrap();
    };
    write("db-1.internal");
    let mut ov = make_overlord(&[], &[]);
    ov.alert_sender.spawn_worker();
    ov.layers_dir = dir.path().to_path_buf();
    ov.reload_layers().await.unwrap();
    let first = ov.checks["c"].handle.tx.clone();

    write("db-2.internal");
    ov.reload_layers().await.unwrap();
    assert!(
        !ov.checks["c"].handle.tx.same_channel(&first),
        "the check actor must be restarted with the new service context"
    );
    assert_eq!(
        ov.checks["c"].context.as_ref().unwrap().environment["DB_HOST"],
        "db-2.internal"
    );
    ov.stop_all().await;
}

// ------------------------------------------------------------------
// Commands from the API, autostart ordering, scheduled runs
// ------------------------------------------------------------------

/// Replace `name`'s actor with a stub that accepts every command and
/// reports which one it got.
fn stub_actor(ov: &mut Overlord, name: &str) -> mpsc::UnboundedReceiver<String> {
    let (tx, mut rx) = mpsc::channel::<service::Cmd>(8);
    let (seen_tx, seen_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            let what = match cmd {
                service::Cmd::Start(r) => {
                    let _ = r.send(Ok(()));
                    "start".to_owned()
                }
                service::Cmd::Stop(r) | service::Cmd::Restart(r) => {
                    let _ = r.send(Ok(()));
                    "stop/restart".to_owned()
                }
                service::Cmd::ForwardSignal(sig) => format!("signal {sig}"),
                _ => "other".to_owned(),
            };
            let _ = seen_tx.send(what);
        }
    });
    ov.services.get_mut(name).unwrap().handle = service::Handle { tx };
    seen_rx
}

async fn services_action(
    ov: &mut Overlord,
    action: vigil_types::api::ServiceAction,
    names: Vec<String>,
) -> vigil_types::api::ChangeInfo {
    let (reply, rx) = tokio::sync::oneshot::channel();
    ov.handle_cmd(super::Cmd::Services {
        action,
        names,
        reply,
    })
    .await;
    rx.await.unwrap().unwrap()
}

async fn next_seen(rx: &mut mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("no command arrived")
        .unwrap()
}

#[tokio::test]
async fn next_run_event_updates_the_snapshot() {
    let mut ov = make_overlord(&[("app", &[], &[], &[])], &[]);
    let at = Utc::now() + chrono::Duration::hours(1);
    svc_event(&mut ov, service::EventKind::NextRun { at: Some(at) }).await;
    assert_eq!(ov.services["app"].snapshot.next_run, Some(at));
    svc_event(&mut ov, service::EventKind::NextRun { at: None }).await;
    assert_eq!(ov.services["app"].snapshot.next_run, None);
}

#[tokio::test]
async fn autostart_defers_a_service_until_its_dependency_runs() {
    let mut ov = make_overlord(&[("db", &[], &[], &[]), ("app", &["db"], &[], &[])], &[]);
    for cfg in ov.plan.services.values_mut() {
        cfg.startup = Some(Startup::Enabled);
    }
    let mut db = stub_actor(&mut ov, "db");
    let mut app = stub_actor(&mut ov, "app");
    ov.autostart().await;
    assert_eq!(next_seen(&mut db).await, "start");
    // db is only starting (no Active event yet): app has to wait.
    assert_eq!(ov.pending_autostart, ["app"]);
    assert!(app.try_recv().is_err());
    // A second autostart (replan) does not queue it twice.
    ov.autostart().await;
    assert_eq!(ov.pending_autostart, ["app"]);
}

/// Plan for the autostart tests: `on` enabled, `off` disabled, `cron`
/// enabled but scheduled, `late` enabled but `requires: [blocker]`, which is
/// disabled and never ready, `ordered` enabled with `after: [blocker]`.
fn autostart_overlord() -> Overlord {
    let mut ov = make_overlord(
        &[
            ("on", &[], &[], &[]),
            ("off", &[], &[], &[]),
            ("cron", &[], &[], &[]),
            ("late", &[], &["blocker"], &[]),
            ("ordered", &["blocker"], &[], &[]),
            ("blocker", &[], &[], &[]),
        ],
        &[],
    );
    for name in ["on", "cron", "late", "ordered"] {
        ov.plan.services.get_mut(name).unwrap().startup = Some(Startup::Enabled);
    }
    ov.plan.services.get_mut("cron").unwrap().schedule = Some("0 3 * * *".into());
    ov
}

/// Wait briefly, then report whether a command reached the stub.
async fn got_command(rx: &mut mpsc::UnboundedReceiver<String>) -> bool {
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    rx.try_recv().is_ok()
}

#[tokio::test]
async fn autostart_action_without_names_starts_what_startup_starts() {
    let mut ov = autostart_overlord();
    let mut on = stub_actor(&mut ov, "on");
    let mut off = stub_actor(&mut ov, "off");
    let mut cron = stub_actor(&mut ov, "cron");
    let mut late = stub_actor(&mut ov, "late");
    let mut ordered = stub_actor(&mut ov, "ordered");
    let change = services_action(&mut ov, vigil_types::api::ServiceAction::Autostart, vec![]).await;
    assert_eq!(change.kind, "autostart");
    assert_eq!(change.status, vigil_types::api::ChangeStatus::Done);
    assert_eq!(next_seen(&mut on).await, "start");
    assert!(!got_command(&mut off).await, "disabled service started");
    assert!(!got_command(&mut cron).await, "scheduled job started");
    assert!(
        !got_command(&mut late).await,
        "started before its required dependency"
    );
    // after: on a service autostart does not start is only an ordering, and
    // it is ignored instead of blocking forever.
    assert_eq!(next_seen(&mut ordered).await, "start");
    assert_eq!(ov.pending_autostart, ["late"]);
    // vigil services list shows what the queued service waits for.
    let infos = ov.get_service_infos(&[]);
    let waiting = |n: &str| {
        infos
            .iter()
            .find(|i| i.name == n)
            .unwrap()
            .waiting_for
            .clone()
    };
    assert_eq!(waiting("late"), ["blocker"]);
    assert!(waiting("ordered").is_empty());
    // The plan is not changed: off stays disabled.
    assert_eq!(ov.plan.services["off"].startup(), Startup::Disabled);

    let (reply, rx) = tokio::sync::oneshot::channel();
    ov.handle_cmd(super::Cmd::GetChanges { id: None, reply })
        .await;
    assert_eq!(rx.await.unwrap().len(), 1);
}

#[tokio::test]
async fn autostart_action_with_names_starts_only_those_it_covers() {
    let mut ov = autostart_overlord();
    let mut on = stub_actor(&mut ov, "on");
    let mut off = stub_actor(&mut ov, "off");
    let names = ["on", "off", "cron", "nope"].map(String::from).to_vec();
    let change = services_action(&mut ov, vigil_types::api::ServiceAction::Autostart, names).await;
    assert_eq!(next_seen(&mut on).await, "start");
    assert!(!got_command(&mut off).await, "disabled service started");
    assert_eq!(change.status, vigil_types::api::ChangeStatus::Error);
    let err = change.err.unwrap();
    for want in [
        "off: startup is not enabled",
        "cron: scheduled; its schedule starts it",
        "nope: not in the plan",
    ] {
        assert!(err.contains(want), "{want}: {err}");
    }
    assert!(!err.starts_with("on:") && !err.contains("; on:"), "{err}");
    assert_eq!(ov.plan.services["off"].startup(), Startup::Disabled);
}

#[tokio::test]
async fn replan_action_reloads_the_layers() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("001.yaml"),
        "services:\n  app:\n    command: sleep 999\n",
    )
    .unwrap();
    let mut ov = make_overlord(&[], &[]);
    ov.layers_dir = dir.path().to_path_buf();
    let change = services_action(
        &mut ov,
        vigil_types::api::ServiceAction::Replan,
        vec!["app".into()],
    )
    .await;
    assert_eq!(change.status, vigil_types::api::ChangeStatus::Done);
    assert!(ov.plan.services.contains_key("app"));
    ov.stop_all().await;
}

#[tokio::test]
async fn forward_signal_command_reaches_every_service() {
    let mut ov = make_overlord(&[("a", &[], &[], &[]), ("b", &[], &[], &[])], &[]);
    let mut a = stub_actor(&mut ov, "a");
    let mut b = stub_actor(&mut ov, "b");
    ov.handle_cmd(super::Cmd::ForwardSignal {
        signal: nix::sys::signal::Signal::SIGHUP,
    })
    .await;
    assert_eq!(next_seen(&mut a).await, "signal SIGHUP");
    assert_eq!(next_seen(&mut b).await, "signal SIGHUP");
}

#[tokio::test]
async fn service_infos_show_done_for_oneshots() {
    let mut ov = make_overlord(&[("job", &[], &[], &[])], &[]);
    ov.plan.services.get_mut("job").unwrap().service_type =
        Some(vigil_types::plan::ServiceType::Oneshot);
    let (reply, rx) = tokio::sync::oneshot::channel();
    ov.handle_cmd(super::Cmd::GetServices {
        names: vec![],
        reply,
    })
    .await;
    assert_eq!(rx.await.unwrap()[0].on_success, "done");
}

// An unchanged HTTP check keeps its actor on replan but gets the client the
// replan built (a rotated tls.ca takes effect); a check without HTTP gets
// nothing.
#[tokio::test]
async fn sync_actors_hands_the_new_http_client_to_unchanged_checks() {
    use super::CheckEntry;
    use crate::check;
    use vigil_types::plan::CheckConfig;
    let mut ov = make_overlord(&[], &[]);
    let web = CheckConfig {
        startup: Some(vigil_types::plan::Startup::Enabled),
        http: Some(vigil_types::plan::HttpCheck {
            url: "http://127.0.0.1:9/".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let exec = CheckConfig {
        startup: Some(vigil_types::plan::Startup::Enabled),
        exec: Some(vigil_types::plan::ExecCheck {
            command: "true".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    ov.plan.checks.insert("web".into(), web.clone());
    ov.plan.checks.insert("cmd".into(), exec.clone());
    ov.check_clients
        .insert("web".into(), reqwest::Client::builder().build().unwrap());

    let mut rxs = Vec::new();
    for (name, config) in [("web", web), ("cmd", exec)] {
        let (tx, rx) = mpsc::channel(1);
        ov.checks.insert(
            name.into(),
            CheckEntry {
                handle: check::Handle { tx },
                config,
                context: None,
            },
        );
        rxs.push(rx);
    }
    ov.sync_actors();
    assert!(matches!(
        rxs[0].try_recv(),
        Ok(check::Cmd::SetHttpClient(_))
    ));
    assert!(rxs[1].try_recv().is_err(), "exec check gets no client");

    // A full channel only logs a warning; the actor is kept.
    let (tx, _rx) = mpsc::channel(1);
    tx.try_send(check::Cmd::Shutdown).unwrap();
    ov.checks.get_mut("web").unwrap().handle = check::Handle { tx };
    ov.sync_actors();
    assert!(ov.checks.contains_key("web"));
}

// after: on a service autostart does not start is still waited for while
// that service is starting (someone started it by hand).
#[tokio::test]
async fn after_waits_for_a_disabled_dependency_that_is_starting() {
    let mut ov = make_overlord(
        &[("app", &["db"], &[], &[]), ("db", &[], &[], &[])],
        &[("db", ServiceState::Starting)],
    );
    ov.plan.services.get_mut("app").unwrap().startup = Some(Startup::Enabled);
    assert_eq!(ov.unmet_deps("app"), ["db"]);
    ov.services.get_mut("db").unwrap().snapshot.state = ServiceState::Inactive;
    assert!(
        ov.unmet_deps("app").is_empty(),
        "inactive and disabled: ignored"
    );
    // before: on the dependency side works the same way.
    let mut ov = make_overlord(&[("app", &[], &[], &[]), ("db", &[], &[], &["app"])], &[]);
    ov.plan.services.get_mut("app").unwrap().startup = Some(Startup::Enabled);
    assert!(ov.unmet_deps("app").is_empty());
    ov.plan.services.get_mut("db").unwrap().startup = Some(Startup::Enabled);
    assert_eq!(ov.unmet_deps("app"), ["db"]);
}
