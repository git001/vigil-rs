// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use indexmap::IndexMap;

use super::*;

fn layer(services: impl IntoIterator<Item = (&'static str, ServiceConfig)>) -> Layer {
    Layer {
        order: 0,
        label: "test".into(),
        summary: None,
        description: None,
        services: services
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
        checks: IndexMap::new(),
        alerts: AlertsBlock::default(),
    }
}

fn svc(command: &str) -> ServiceConfig {
    ServiceConfig {
        command: Some(command.into()),
        ..Default::default()
    }
}

// --- single layer ---

#[test]
fn empty_layers_produces_empty_plan() {
    let plan = Plan::from_layers(vec![]);
    assert!(plan.services.is_empty());
    assert!(plan.checks.is_empty());
}

#[test]
fn single_layer_is_passed_through() {
    let plan = Plan::from_layers(vec![layer([("app", svc("/bin/app"))])]);
    assert_eq!(plan.services["app"].command.as_deref(), Some("/bin/app"));
}

// --- merge: command override ---

#[test]
fn later_layer_overrides_command() {
    let base = layer([("app", svc("/bin/v1"))]);
    let overlay = layer([("app", svc("/bin/v2"))]);
    let plan = Plan::from_layers(vec![base, overlay]);
    assert_eq!(plan.services["app"].command.as_deref(), Some("/bin/v2"));
}

#[test]
fn overlay_none_field_does_not_clear_base() {
    let mut base_svc = svc("/bin/app");
    base_svc.user = Some("alice".into());
    let overlay_svc = svc("/bin/app-v2"); // user is None
    let plan = Plan::from_layers(vec![
        layer([("app", base_svc)]),
        layer([("app", overlay_svc)]),
    ]);
    // user from base must survive
    assert_eq!(plan.services["app"].user.as_deref(), Some("alice"));
}

// --- merge: lists are union-merged, no duplicates ---

#[test]
fn after_lists_are_merged_without_duplicates() {
    let mut s1 = svc("/bin/app");
    s1.after = vec!["db".into(), "cache".into()];
    let mut s2 = svc("/bin/app");
    s2.after = vec!["cache".into(), "queue".into()]; // "cache" already in base
    let plan = Plan::from_layers(vec![layer([("app", s1)]), layer([("app", s2)])]);
    let after = &plan.services["app"].after;
    assert_eq!(after.len(), 3, "expected db, cache, queue — got {after:?}");
    assert!(after.contains(&"db".to_owned()));
    assert!(after.contains(&"cache".to_owned()));
    assert!(after.contains(&"queue".to_owned()));
}

#[test]
fn requires_lists_are_merged() {
    let mut s1 = svc("/bin/app");
    s1.requires = vec!["db".into()];
    let mut s2 = svc("/bin/app");
    s2.requires = vec!["auth".into()];
    let plan = Plan::from_layers(vec![layer([("app", s1)]), layer([("app", s2)])]);
    let req = &plan.services["app"].requires;
    assert!(req.contains(&"db".to_owned()));
    assert!(req.contains(&"auth".to_owned()));
}

// --- merge: environment map ---

#[test]
fn environment_maps_are_merged_overlay_wins() {
    let mut s1 = svc("/bin/app");
    s1.environment.insert("FOO".into(), "base".into());
    s1.environment.insert("BAR".into(), "base".into());
    let mut s2 = svc("/bin/app");
    s2.environment.insert("FOO".into(), "overlay".into()); // override
    s2.environment.insert("BAZ".into(), "overlay".into()); // new key
    let plan = Plan::from_layers(vec![layer([("app", s1)]), layer([("app", s2)])]);
    let env = &plan.services["app"].environment;
    assert_eq!(env["FOO"], "overlay"); // overlay wins
    assert_eq!(env["BAR"], "base"); // base survives
    assert_eq!(env["BAZ"], "overlay"); // new key added
}

// --- replace override ---

#[test]
fn replace_override_discards_base() {
    let mut s1 = svc("/bin/v1");
    s1.after = vec!["db".into()];
    s1.environment.insert("KEY".into(), "val".into());

    let mut s2 = svc("/bin/v2");
    s2.override_mode = Override::Replace;

    let plan = Plan::from_layers(vec![layer([("app", s1)]), layer([("app", s2)])]);
    let svc = &plan.services["app"];
    assert_eq!(svc.command.as_deref(), Some("/bin/v2"));
    assert!(svc.after.is_empty(), "replace must clear after list");
    assert!(svc.environment.is_empty(), "replace must clear environment");
}

// --- multiple services, independent ---

#[test]
fn multiple_services_are_independent() {
    let plan = Plan::from_layers(vec![layer([
        ("web", svc("/bin/web")),
        ("db", svc("/bin/db")),
    ])]);
    assert_eq!(plan.services.len(), 2);
    assert_eq!(plan.services["web"].command.as_deref(), Some("/bin/web"));
    assert_eq!(plan.services["db"].command.as_deref(), Some("/bin/db"));
}

// --- alert layer merging ---

fn alert_layer(name: &'static str, alert: AlertConfig) -> Layer {
    Layer {
        order: 0,
        label: "test".into(),
        summary: None,
        description: None,
        services: IndexMap::new(),
        checks: IndexMap::new(),
        alerts: AlertsBlock {
            entries: [(name.to_owned(), alert)].into_iter().collect(),
            ..Default::default()
        },
    }
}

fn alert(url: &str) -> AlertConfig {
    AlertConfig {
        url: url.into(),
        ..Default::default()
    }
}

#[test]
fn alert_added_by_layer() {
    let plan = Plan::from_layers(vec![alert_layer(
        "webhook",
        alert("http://example.com/hook"),
    )]);
    assert!(plan.alerts.contains_key("webhook"));
    assert_eq!(plan.alerts["webhook"].url, "http://example.com/hook");
}

#[test]
fn alert_merge_updates_url() {
    let a1 = alert("http://old.example.com");
    let a2 = alert("http://new.example.com");
    let plan = Plan::from_layers(vec![alert_layer("hook", a1), alert_layer("hook", a2)]);
    assert_eq!(plan.alerts["hook"].url, "http://new.example.com");
}

#[test]
fn alert_merge_adds_labels() {
    let mut a1 = alert("http://example.com");
    a1.labels.insert("env".into(), "prod".into());
    let mut a2 = alert("http://example.com");
    a2.labels.insert("cluster".into(), "k8s".into());
    let plan = Plan::from_layers(vec![alert_layer("hook", a1), alert_layer("hook", a2)]);
    let labels = &plan.alerts["hook"].labels;
    assert_eq!(labels["env"], "prod");
    assert_eq!(labels["cluster"], "k8s");
}

#[test]
fn alert_merge_deduplicates_on_check() {
    let mut a1 = alert("http://example.com");
    a1.on_check = vec!["check-a".into(), "check-b".into()];
    let mut a2 = alert("http://example.com");
    a2.on_check = vec!["check-b".into(), "check-c".into()]; // "check-b" duplicate
    let plan = Plan::from_layers(vec![alert_layer("hook", a1), alert_layer("hook", a2)]);
    let on_check = &plan.alerts["hook"].on_check;
    assert_eq!(
        on_check.len(),
        3,
        "expected check-a, check-b, check-c — got {on_check:?}"
    );
}

#[test]
fn alert_replace_override_discards_base() {
    let base = alert("http://old.example.com");
    let mut overlay = alert("http://new.example.com");
    overlay.override_mode = Override::Replace;
    overlay.on_check = vec!["new-check".into()];
    let plan = Plan::from_layers(vec![
        alert_layer("hook", base),
        alert_layer("hook", overlay),
    ]);
    assert_eq!(plan.alerts["hook"].url, "http://new.example.com");
    assert_eq!(plan.alerts["hook"].on_check, vec!["new-check".to_string()]);
}

#[test]
fn alertconfig_serde_roundtrip() {
    let yaml = r#"
url: http://example.com/hook
format: alertmanager
on-check: [check-a, check-b]
headers:
  Authorization: Bearer token
labels:
  cluster: prod
retry-attempts: 5
retry-backoff: [1s, 2s, 4s]
"#;
    let cfg: AlertConfig = serde_yaml::from_str(yaml).expect("parse failed");
    assert_eq!(cfg.url, "http://example.com/hook");
    assert_eq!(cfg.format, AlertFormat::Alertmanager);
    assert_eq!(cfg.on_check, vec!["check-a", "check-b"]);
    assert_eq!(cfg.headers["Authorization"], "Bearer token");
    assert_eq!(cfg.labels["cluster"], "prod");
    assert_eq!(cfg.retry_attempts, Some(5));
    assert_eq!(cfg.retry_backoff, vec!["1s", "2s", "4s"]);
}

#[test]
fn alertformat_all_variants_deserialize() {
    assert_eq!(
        serde_yaml::from_str::<AlertFormat>("webhook").unwrap(),
        AlertFormat::Webhook
    );
    assert_eq!(
        serde_yaml::from_str::<AlertFormat>("alertmanager").unwrap(),
        AlertFormat::Alertmanager
    );
    assert_eq!(
        serde_yaml::from_str::<AlertFormat>("cloud-events").unwrap(),
        AlertFormat::CloudEvents
    );
    assert_eq!(
        serde_yaml::from_str::<AlertFormat>("otlp-logs").unwrap(),
        AlertFormat::OtlpLogs
    );
}

// --- startup field ---

#[test]
fn startup_enabled_propagates() {
    let mut s = svc("/bin/app");
    s.startup = Startup::Enabled;
    let plan = Plan::from_layers(vec![layer([("app", s)])]);
    assert_eq!(plan.services["app"].startup, Startup::Enabled);
}

#[test]
fn startup_can_be_overridden_to_disabled() {
    let mut s1 = svc("/bin/app");
    s1.startup = Startup::Enabled;
    let mut s2 = svc("/bin/app");
    s2.startup = Startup::Disabled;
    let plan = Plan::from_layers(vec![layer([("app", s1)]), layer([("app", s2)])]);
    assert_eq!(plan.services["app"].startup, Startup::Disabled);
}

// --- check layer merging ---

fn check_layer(name: &'static str, chk: CheckConfig) -> Layer {
    Layer {
        order: 0,
        label: "test".into(),
        summary: None,
        description: None,
        services: IndexMap::new(),
        checks: [(name.to_owned(), chk)].into_iter().collect(),
        alerts: AlertsBlock::default(),
    }
}

fn chk(period: &str) -> CheckConfig {
    CheckConfig {
        period: Some(period.into()),
        ..Default::default()
    }
}

#[test]
fn check_added_by_layer() {
    let plan = Plan::from_layers(vec![check_layer("http-alive", chk("10s"))]);
    assert!(plan.checks.contains_key("http-alive"));
    assert_eq!(plan.checks["http-alive"].period.as_deref(), Some("10s"));
}

#[test]
fn check_merge_updates_period() {
    let plan = Plan::from_layers(vec![
        check_layer("http-alive", chk("10s")),
        check_layer("http-alive", chk("30s")),
    ]);
    assert_eq!(plan.checks["http-alive"].period.as_deref(), Some("30s"));
}

#[test]
fn check_replace_override_discards_base() {
    let base = chk("10s");
    let mut overlay = chk("30s");
    overlay.override_mode = Override::Replace;
    let plan = Plan::from_layers(vec![
        check_layer("http-alive", base),
        check_layer("http-alive", overlay),
    ]);
    assert_eq!(plan.checks["http-alive"].period.as_deref(), Some("30s"));
}

// --- alert queue settings ---

#[test]
fn alert_queue_depth_last_layer_wins() {
    let l1 = Layer {
        alerts: AlertsBlock {
            max_queue_depth: Some(64),
            ..Default::default()
        },
        ..Default::default()
    };
    let l2 = Layer {
        alerts: AlertsBlock {
            max_queue_depth: Some(128),
            ..Default::default()
        },
        ..Default::default()
    };
    let plan = Plan::from_layers(vec![l1, l2]);
    assert_eq!(plan.alert_queue_depth, Some(128));
}

#[test]
fn alert_max_queue_time_last_layer_wins() {
    let l1 = Layer {
        alerts: AlertsBlock {
            max_queue_time: Some("30s".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let l2 = Layer {
        alerts: AlertsBlock {
            max_queue_time: Some("2m".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let plan = Plan::from_layers(vec![l1, l2]);
    assert_eq!(plan.alert_max_queue_time.as_deref(), Some("2m"));
}
