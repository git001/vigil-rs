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
        identities: IndexMap::new(),
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
        identities: IndexMap::new(),
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
fn alert_merge_keeps_format_and_tls_when_not_set() {
    let mut base = alert("http://example.com");
    base.format = Some(AlertFormat::Alertmanager);
    base.tls = Some(TlsConfig {
        ca: vec!["/etc/vigil/ca.pem".into()],
        insecure: true,
        ..Default::default()
    });
    let mut overlay = alert("http://example.com");
    overlay.labels.insert("cluster".into(), "prod".into());
    let plan = Plan::from_layers(vec![
        alert_layer("hook", base),
        alert_layer("hook", overlay),
    ]);
    let a = &plan.alerts["hook"];
    assert_eq!(a.format(), AlertFormat::Alertmanager);
    assert!(a.tls.as_ref().unwrap().insecure);
    assert_eq!(a.labels["cluster"], "prod");

    // Set in a later layer: the whole block is replaced.
    let mut overlay = alert("http://example.com");
    overlay.format = Some(AlertFormat::Webhook);
    overlay.tls = Some(TlsConfig {
        cert: Some("/run/secrets/vigil/client.crt".into()),
        key: Some("/run/secrets/vigil/client.key".into()),
        ..Default::default()
    });
    let plan = Plan::from_layers(vec![
        alert_layer("hook", plan.alerts["hook"].clone()),
        alert_layer("hook", overlay),
    ]);
    let a = &plan.alerts["hook"];
    assert_eq!(a.format(), AlertFormat::Webhook);
    let tls = a.tls.as_ref().unwrap();
    assert!(!tls.insecure && tls.ca.is_empty() && tls.cert.is_some());
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
    assert_eq!(cfg.format(), AlertFormat::Alertmanager);
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
    s.startup = Some(Startup::Enabled);
    let plan = Plan::from_layers(vec![layer([("app", s)])]);
    assert_eq!(plan.services["app"].startup(), Startup::Enabled);
}

#[test]
fn startup_can_be_overridden_to_disabled() {
    let mut s1 = svc("/bin/app");
    s1.startup = Some(Startup::Enabled);
    let mut s2 = svc("/bin/app");
    s2.startup = Some(Startup::Disabled);
    let plan = Plan::from_layers(vec![layer([("app", s1)]), layer([("app", s2)])]);
    assert_eq!(plan.services["app"].startup(), Startup::Disabled);
}

/// A layer that does not mention `startup` must not reset it (pebble
/// semantics); before v4 it silently became `disabled`.
#[test]
fn startup_is_kept_when_a_later_layer_does_not_set_it() {
    let mut s1 = svc("/bin/app");
    s1.startup = Some(Startup::Enabled);
    let mut s2 = svc("/bin/app");
    s2.environment.insert("X".into(), "1".into());
    let plan = Plan::from_layers(vec![layer([("app", s1)]), layer([("app", s2)])]);
    assert_eq!(plan.services["app"].startup(), Startup::Enabled);
    assert_eq!(plan.services["app"].environment["X"], "1");
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
        identities: IndexMap::new(),
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

/// `override: replace` is read from YAML under the documented key (it was
/// silently ignored as an unknown field before v4).
#[test]
fn override_replace_is_read_from_yaml() {
    let l1: Layer = serde_yaml::from_str(
        "services:\n  app:\n    command: /usr/bin/app --old\n    environment:\n      MODE: old\n",
    )
    .unwrap();
    let l2: Layer = serde_yaml::from_str(
        "services:\n  app:\n    override: replace\n    command: /usr/bin/app --new\n",
    )
    .unwrap();
    let plan = Plan::from_layers(vec![l1, l2]);
    let app = &plan.services["app"];
    assert_eq!(app.command.as_deref(), Some("/usr/bin/app --new"));
    assert!(
        app.environment.is_empty(),
        "replace drops the old environment"
    );
}

#[test]
fn unknown_fields_are_rejected_with_the_field_name() {
    for (yaml, field) in [
        ("services:\n  app:\n    comand: /bin/true\n", "comand"),
        ("checks:\n  c:\n    perod: 5s\n", "perod"),
        ("checks:\n  c:\n    http:\n      ulr: http://x/\n", "ulr"),
        (
            "alerts:\n  a:\n    url: http://x/\n    retry_attempts: 5\n",
            "retry_attempts",
        ),
        ("servics: {}\n", "servics"),
    ] {
        let err = serde_yaml::from_str::<Layer>(yaml).unwrap_err().to_string();
        assert!(
            err.contains(&format!("unknown field `{field}`")),
            "{yaml}: {err}"
        );
    }
}

#[test]
fn service_merge_rules() {
    let base: Layer = serde_yaml::from_str(
        "services:\n  app:\n    command: /usr/bin/app\n    after: [db]\n    environment:\n      MODE: prod\n      LOG: info\n    success-exit-codes: [0, 3]\n    exit-code-map: {143: 0}\n    on-check-failure: {alive: restart}\n    user: app\n",
    )
    .unwrap();
    let overlay: Layer = serde_yaml::from_str(
        "services:\n  app:\n    after: [db, cache]\n    environment:\n      LOG: debug\n    success-exit-codes: [0]\n    exit-code-map: {137: 1}\n    on-check-failure: {ready: ignore}\n    kill-delay: 10s\n",
    )
    .unwrap();
    let plan = Plan::from_layers(vec![base, overlay]);
    let app = &plan.services["app"];
    assert_eq!(app.command.as_deref(), Some("/usr/bin/app"), "unset: kept");
    assert_eq!(app.user.as_deref(), Some("app"));
    assert_eq!(app.kill_delay.as_deref(), Some("10s"), "set: taken");
    assert_eq!(app.after, ["db", "cache"], "lists: appended, no duplicates");
    assert_eq!(app.environment["MODE"], "prod", "maps: extended");
    assert_eq!(app.environment["LOG"], "debug", "maps: overlay wins");
    assert_eq!(app.success_exit_codes, [0], "success-exit-codes: replaced");
    assert_eq!(app.exit_code_map.len(), 2);
    assert_eq!(app.on_check_failure.len(), 2);
}

#[test]
fn alerts_block_reads_settings_and_alerts() {
    let block: AlertsBlock =
        serde_yaml::from_str("max-queue-depth: 5\nmax-queue-time: 30s\nhook:\n  url: http://x/\n")
            .unwrap();
    assert_eq!(block.max_queue_depth, Some(5));
    assert_eq!(block.max_queue_time.as_deref(), Some("30s"));
    assert_eq!(block.entries["hook"].url, "http://x/");
}

#[test]
fn alerts_block_errors_name_the_key() {
    let err = serde_yaml::from_str::<AlertsBlock>("max-queue-depht: 5\n")
        .unwrap_err()
        .to_string();
    assert!(err.contains("expected an alert (a mapping)"), "{err}");
    assert!(
        err.contains("\"max-queue-depht\" is not a setting either"),
        "{err}"
    );

    let err = serde_yaml::from_str::<AlertsBlock>("hook:\n  url: http://x/\n  urll: y\n")
        .unwrap_err()
        .to_string();
    assert!(err.starts_with("hook: unknown field `urll`"), "{err}");
}

#[test]
fn to_layer_reads_back_as_the_same_plan() {
    let parse = |yaml: &str| -> Layer { serde_yaml::from_str(yaml).unwrap() };
    let plan = Plan::from_layers(vec![
        parse(
            "services:\n  web: {command: a, environment: {A: '1'}}\n\
             checks:\n  up: {tcp: {port: 80}}\n\
             alerts:\n  max-queue-depth: 10\n  ops: {url: 'https://x', on-check: [up]}\n\
             identities:\n  me: {access: admin, local: {}}\n",
        ),
        parse(
            "services:\n  web: {override: merge, environment: {B: '2'}}\n  \
             db: {override: replace, command: b}\n",
        ),
    ]);
    let layer = plan.to_layer();
    let yaml = serde_yaml::to_string(&layer).unwrap();
    assert!(!yaml.contains("override"), "{yaml}");
    let again = Plan::from_layers(vec![parse(&yaml)]);
    assert_eq!(again.services, plan.services_without_override());
    assert_eq!(again.checks, plan.checks);
    assert_eq!(again.alerts, plan.alerts);
    assert_eq!(again.alert_queue_depth, Some(10));
    assert_eq!(again.identities["me"].access, plan.identities["me"].access);

    let empty = serde_yaml::to_string(&Plan::from_layers(vec![]).to_layer()).unwrap();
    assert_eq!(empty, "{}\n");
}

impl Plan {
    /// The services with `override` reset, as [`Plan::to_layer`] writes them.
    fn services_without_override(&self) -> IndexMap<String, ServiceConfig> {
        self.services
            .iter()
            .map(|(n, s)| {
                (
                    n.clone(),
                    ServiceConfig {
                        override_mode: Override::Merge,
                        ..s.clone()
                    },
                )
            })
            .collect()
    }
}
