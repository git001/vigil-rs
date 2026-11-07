// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::fs;

use tempfile::TempDir;
use vigil_types::plan::CheckConfig;

use super::check::level_warning;
use super::dependencies::dependency_warnings;
use super::*;

fn load_plan(dir: &Path) -> anyhow::Result<Plan> {
    load(dir).map(|l| l.plan)
}

fn write(dir: &TempDir, name: &str, content: &str) {
    fs::write(dir.path().join(name), content).unwrap();
}

#[test]
fn later_layer_replaces_a_whole_identity() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001-base.yaml",
        "identities:\n  ops:\n    access: admin\n    local: {}\n",
    );
    write(
        &dir,
        "002-site.yaml",
        "identities:\n  ops:\n    access: read\n    local:\n      user-id: 1000\n",
    );
    let loaded = load(dir.path()).unwrap();
    let ops = &loaded.identities[0];
    assert_eq!(ops.access, vigil_types::identity::IdentityAccess::Read);
    assert_eq!(ops.local.as_ref().unwrap().user_id, Some(1000));
    assert_eq!(ops.layer.as_deref(), Some("002-site"));
}

#[test]
fn identities_reject_override_and_unknown_keys() {
    for (yaml, want) in [
        (
            "identities:\n  ops:\n    override: replace\n    access: admin\n    local: {}\n",
            "unknown field `override`",
        ),
        (
            "identities:\n  ops:\n    access: admin\n    basic:\n      password: x\n",
            "unknown field `password`",
        ),
        (
            "identities:\n  ops:\n    access: superuser\n    local: {}\n",
            "unknown variant `superuser`",
        ),
    ] {
        let dir = TempDir::new().unwrap();
        write(&dir, "001.yaml", yaml);
        let err = format!("{:#}", load_plan(dir.path()).unwrap_err());
        assert!(err.contains(want), "{yaml}: {err}");
    }
}

#[test]
fn unset_alert_env_variable_rejects_plan() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001.yaml",
        "alerts:\n  ops:\n    url: \"env:VIGIL_TEST_UNSET_WEBHOOK_URL\"\n    pass-env-glob: [VIGIL_TEST_*]\n",
    );
    let err = format!("{:#}", load_plan(dir.path()).unwrap_err());
    assert!(
        err.contains(
            "alert \"ops\": url: environment variable VIGIL_TEST_UNSET_WEBHOOK_URL is not set"
        ),
        "{err}"
    );
}

/// The variables the examples reference, as their README sets them. The
/// examples load as shipped: no file they name may be missing.
fn examples_env() -> crate::alert::LoadEnv {
    let parent = [
        (
            "SLACK_WEBHOOK_URL",
            "https://hooks.slack.com/services/T000/B000/XXXX",
        ),
        (
            "TEAMS_WEBHOOK_URL",
            "https://example.webhook.office.com/webhookb2/xxx",
        ),
        ("WEBHOOK_URL", "http://my-webhook-endpoint:8080/alert"),
        ("WEBHOOK_AUTH", "Bearer eyJhbGciOiJIUzI1NiJ9"),
        ("ALERTMANAGER_URL", "http://alertmanager:9093/api/v2/alerts"),
        ("ALERTMANAGER_AUTH", "Bearer eyJhbGciOiJIUzI1NiJ9"),
        ("CLUSTER_NAME", "prod-eu-1"),
        ("KUBERNETES_NAMESPACE", "shop"),
        ("KUBERNETES_SERVICE_NAME", "api"),
        ("HTTPS_PROXY", "http://corp-proxy.example.com:3128"),
        ("NO_PROXY", "localhost,.internal"),
    ];
    crate::alert::LoadEnv {
        parent: parent.iter().map(|(k, v)| (k.into(), v.into())).collect(),
    }
}

fn load_err(yaml: &str) -> String {
    let dir = TempDir::new().unwrap();
    write(&dir, "001.yaml", yaml);
    format!("{:#}", load_plan(dir.path()).unwrap_err())
}

#[test]
fn dependency_names_must_be_other_services() {
    for (key, want) in [
        ("after: [dbb]", "after: \"dbb\" is not a service"),
        ("before: [dbb]", "before: \"dbb\" is not a service"),
        ("requires: [dbb]", "requires: \"dbb\" is not a service"),
        ("after: [app]", "after: a service cannot depend on itself"),
    ] {
        let err = load_err(&format!(
            "services:\n  db:\n    command: sleep 9\n  app:\n    command: sleep 9\n    {key}\n"
        ));
        assert!(err.contains("service \"app\""), "{err}");
        assert!(err.contains(want), "{key}: {err}");
    }
}

#[test]
fn dependency_cycles_are_rejected_with_the_chain() {
    // after and requires.
    let err = load_err(
        "services:\n  a:\n    command: sleep 9\n    after: [b]\n  b:\n    command: sleep 9\n    requires: [c]\n  c:\n    command: sleep 9\n    after: [a]\n",
    );
    assert!(err.contains("services wait for each other"), "{err}");
    assert!(err.contains("a → b → c → a"), "{err}");
    // before: b before a and a after b is the same direction (fine);
    // a before b and a after b is a cycle.
    let err = load_err(
        "services:\n  a:\n    command: sleep 9\n    after: [b]\n    before: [b]\n  b:\n    command: sleep 9\n",
    );
    assert!(err.contains("a → b → a"), "{err}");
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001.yaml",
        "services:\n  a:\n    command: sleep 9\n    after: [b]\n  b:\n    command: sleep 9\n    before: [a]\n  c:\n    command: sleep 9\n    requires: [a, b]\n",
    );
    load_plan(dir.path()).unwrap();
}

#[test]
fn a_check_needs_exactly_one_probe() {
    let err = load_err("checks:\n  empty:\n    period: 10s\n");
    assert!(err.contains("check \"empty\""), "{err}");
    assert!(
        err.contains("needs exactly one of http, tcp or exec"),
        "{err}"
    );
    let err = load_err(
        "checks:\n  both:\n    http:\n      url: http://127.0.0.1:8080/\n    tcp:\n      port: 8080\n",
    );
    assert!(
        err.contains("needs exactly one of http, tcp or exec"),
        "{err}"
    );
}

#[test]
fn exec_service_context_must_be_a_service() {
    let err = load_err(
        "checks:\n  db-up:\n    exec:\n      command: pg_isready\n      service-context: postgress\n",
    );
    assert!(err.contains("check \"db-up\""), "{err}");
    assert!(
        err.contains("exec: service-context \"postgress\" is not a service"),
        "{err}"
    );
}

fn check_of(yaml: &str) -> CheckConfig {
    let layer: Layer = serde_yaml::from_str(&format!("checks:\n  c:\n{yaml}")).unwrap();
    Plan::from_layers(vec![layer]).checks["c"].clone()
}

#[test]
fn level_warning_for_alive_checks_of_other_hosts() {
    // Another host: warned, naming the default level when it is not set.
    let w = level_warning(&check_of(
        "    startup: enabled\n    tcp:\n      host: postgres.db.svc.cluster.local\n      port: 5432\n",
    ))
    .unwrap();
    assert!(
        w.starts_with(
            "level: alive (the default), but it probes \"postgres.db.svc.cluster.local:5432\""
        ),
        "{w}"
    );
    assert!(w.contains("set level: ready"), "{w}");
    let w = level_warning(&check_of(
        "    level: alive\n    startup: enabled\n    http:\n      url: https://auth.example.com/health\n",
    ))
    .unwrap();
    assert!(
        w.starts_with("level: alive, but it probes \"auth.example.com\""),
        "{w}"
    );
}

#[test]
fn no_level_warning_for_this_container_ready_disabled_or_exec() {
    for yaml in [
        // This container, by name and address.
        "    startup: enabled\n    http:\n      url: http://localhost:8080/actuator/health/liveness\n",
        "    startup: enabled\n    http:\n      url: http://127.0.0.1:9091/healthz\n",
        "    startup: enabled\n    http:\n      url: http://[::1]:8080/\n",
        "    startup: enabled\n    http:\n      url: http://app.localhost:8080/\n",
        "    startup: enabled\n    tcp:\n      port: 5432\n",
        "    startup: enabled\n    tcp:\n      host: 0.0.0.0\n      port: 5432\n",
        // A dependency checked as ready, a disabled check, an exec check.
        "    level: ready\n    startup: enabled\n    tcp:\n      host: db\n      port: 5432\n",
        "    startup: disabled\n    tcp:\n      host: db\n      port: 5432\n",
        "    startup: enabled\n    exec:\n      command: pg_isready -h db\n",
    ] {
        assert_eq!(level_warning(&check_of(yaml)), None, "{yaml}");
    }
}

#[test]
fn dependency_warnings_name_ignored_orderings_and_waiting_requires() {
    let layer: Layer = serde_yaml::from_str(
        "services:\n  app:\n    command: sleep 9\n    startup: enabled\n    after: [db, cache]\n    requires: [db]\n  db:\n    command: sleep 9\n  cache:\n    command: sleep 9\n    startup: enabled\n  backup:\n    command: sleep 9\n    startup: enabled\n    schedule: \"0 3 * * *\"\n    before: [app]\n  idle:\n    command: sleep 9\n    after: [db]\n",
    )
    .unwrap();
    let plan = Plan::from_layers(vec![layer]);
    let w = dependency_warnings(&plan, "app", &plan.services["app"]);
    assert_eq!(w.len(), 3, "{w:?}");
    assert!(w[0].contains("after \"db\"") && w[0].contains("startup is not enabled"));
    assert!(w[0].contains("the ordering is ignored"));
    assert!(
        w[1].contains("after \"backup\"") && w[1].contains("scheduled; its schedule starts it")
    );
    assert!(w[2].contains("requires \"db\"") && w[2].contains("\"app\" waits until \"db\""));
    // A service autostart does not start itself gets no warnings.
    assert!(dependency_warnings(&plan, "idle", &plan.services["idle"]).is_empty());
}

#[test]
fn referenced_files_cover_tls_of_checks_and_alerts_once() {
    let layer: Layer = serde_yaml::from_str(
        "checks:\n  web:\n    http:\n      url: https://example.com/\n      tls:\n        ca: [/etc/vigil/ca/a.pem, /etc/vigil/ca/b.pem]\n        cert: /run/secrets/vigil/client.crt\n        key: /run/secrets/vigil/client.key\n  plain:\n    http:\n      url: https://example.com/\nalerts:\n  am:\n    url: https://alertmanager:9093/api/v2/alerts\n    tls:\n      ca: /etc/vigil/ca/a.pem\n",
    )
    .unwrap();
    let plan = Plan::from_layers(vec![layer]);
    let files = referenced_files(&plan, vec!["/run/secrets/vigil/log-relay.hash".into()]);
    let want: Vec<PathBuf> = [
        "/run/secrets/vigil/log-relay.hash",
        "/etc/vigil/ca/a.pem",
        "/etc/vigil/ca/b.pem",
        "/run/secrets/vigil/client.crt",
        "/run/secrets/vigil/client.key",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    assert_eq!(files, want);
}

/// Every example's layers must load (and pass validation), so example
/// configs cannot silently break, e.g. through new pass-env rules.
#[test]
fn all_example_layers_load() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut checked = 0;
    for entry in fs::read_dir(&examples).unwrap() {
        let layers = entry.unwrap().path().join("layers");
        if layers.is_dir() {
            load_with(&layers, &examples_env())
                .map(|_| ())
                .unwrap_or_else(|e| panic!("{layers:?}: {e:#}"));
            checked += 1;
        }
    }
    // examples/layers and examples/layers-minimal have no layers/
    // subdir; they are layers dirs themselves.
    for name in ["layers", "layers-minimal"] {
        let dir = examples.join(name);
        load_with(&dir, &examples_env())
            .map(|_| ())
            .unwrap_or_else(|e| panic!("{dir:?}: {e:#}"));
        checked += 1;
    }
    assert!(checked > 5, "only {checked} example layer dirs found");
}

#[test]
fn invalid_pass_env_regex_rejects_plan() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001.yaml",
        "services:\n  app:\n    command: /bin/true\n    pass-env-regex: ['(']\n",
    );
    let err = format!("{:#}", load_plan(dir.path()).unwrap_err());
    assert!(err.contains("service \"app\""), "{err}");
    assert!(err.contains("invalid pass-env-regex"), "{err}");
}

/// Redundant lists only warn (see `redundant_lists`); the layer loads.
#[test]
fn pass_env_true_with_globs_loads() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001.yaml",
        "services:\n  app:\n    command: /bin/true\n    pass-env: true\n    pass-env-glob: [APP_*]\n",
    );
    assert!(load_plan(dir.path()).is_ok());
}

/// `pass-env: true` on the check overrides the context's lists.
#[test]
fn exec_check_pass_env_true_in_a_service_context_with_lists_loads() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001.yaml",
        "services:\n  app:\n    command: /bin/true\n    pass-env-glob: [APP_*]\n\
         checks:\n  c:\n    exec:\n      command: 'true'\n      service-context: app\n      pass-env: true\n",
    );
    assert!(load_plan(dir.path()).is_ok());
}

#[test]
fn pass_env_fields_parse_and_merge() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001.yaml",
        "services:\n  app:\n    command: /bin/true\n    pass-env-glob: [APP_*]\n",
    );
    write(
        &dir,
        "002.yaml",
        "services:\n  app:\n    override: merge\n    pass-env-glob: [DB_*]\n    pass-env-regex: ['^K8S_.*']\n",
    );
    let plan = load_plan(dir.path()).unwrap();
    let app = &plan.services["app"];
    assert_eq!(app.pass_env_glob, vec!["APP_*", "DB_*"]);
    assert_eq!(app.pass_env_regex, vec!["^K8S_.*"]);
    assert_eq!(app.pass_env, None);
}

#[test]
fn nonexistent_dir_returns_empty_plan() {
    let plan = load_plan(Path::new("/tmp/vigil-test-nonexistent-99999")).unwrap();
    assert!(plan.services.is_empty());
}

#[test]
fn empty_dir_returns_empty_plan() {
    let dir = TempDir::new().unwrap();
    let plan = load_plan(dir.path()).unwrap();
    assert!(plan.services.is_empty());
}

#[test]
fn single_valid_layer_parsed() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001-app.yaml",
        "services:\n  myapp:\n    command: /bin/echo hello\n",
    );
    let plan = load_plan(dir.path()).unwrap();
    assert!(plan.services.contains_key("myapp"));
    assert_eq!(
        plan.services["myapp"].command.as_deref().unwrap(),
        "/bin/echo hello"
    );
}

#[test]
fn layers_sorted_by_filename() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "002-b.yaml",
        "services:\n  svc:\n    command: /bin/b\n",
    );
    write(
        &dir,
        "001-a.yaml",
        "services:\n  svc:\n    command: /bin/a\n",
    );
    // 002 should override 001 because sorting is by filename
    let plan = load_plan(dir.path()).unwrap();
    assert_eq!(plan.services["svc"].command.as_deref().unwrap(), "/bin/b");
}

#[test]
fn non_yaml_files_are_ignored() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001-app.yaml",
        "services:\n  myapp:\n    command: /bin/echo\n",
    );
    write(&dir, "README.md", "# not yaml");
    write(&dir, "config.json", "{}");
    let plan = load_plan(dir.path()).unwrap();
    assert_eq!(plan.services.len(), 1);
    assert!(plan.services.contains_key("myapp"));
}

#[test]
fn yml_extension_is_accepted() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001-app.yml",
        "services:\n  myapp:\n    command: /bin/echo\n",
    );
    let plan = load_plan(dir.path()).unwrap();
    assert!(plan.services.contains_key("myapp"));
}

#[test]
fn invalid_yaml_returns_error() {
    let dir = TempDir::new().unwrap();
    write(&dir, "001-bad.yaml", "services: [\ninvalid yaml {{{\n");
    let err = load_plan(dir.path());
    assert!(err.is_err());
    let msg = format!("{}", err.unwrap_err());
    assert!(msg.contains("001-bad.yaml") || msg.contains("parsing"));
}

#[test]
fn multiple_services_across_layers() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001-base.yaml",
        "services:\n  alpha:\n    command: /bin/alpha\n  beta:\n    command: /bin/beta\n",
    );
    write(
        &dir,
        "002-extra.yaml",
        "services:\n  gamma:\n    command: /bin/gamma\n",
    );
    let plan = load_plan(dir.path()).unwrap();
    assert!(plan.services.contains_key("alpha"));
    assert!(plan.services.contains_key("beta"));
    assert!(plan.services.contains_key("gamma"));
}

#[test]
fn layer_order_field_set_by_position() {
    // Test that layers have increasing order values based on filename sort
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001-first.yaml",
        "services:\n  svc:\n    command: /bin/first\n",
    );
    write(
        &dir,
        "002-second.yaml",
        "services:\n  svc:\n    command: /bin/second\n",
    );
    // We can't inspect layer.order directly via Plan, but we can verify
    // that the latter layer wins (order-based merge)
    let plan = load_plan(dir.path()).unwrap();
    assert_eq!(
        plan.services["svc"].command.as_deref().unwrap(),
        "/bin/second"
    );
}

#[test]
fn empty_yaml_file_returns_empty_plan() {
    let dir = TempDir::new().unwrap();
    write(&dir, "001-empty.yaml", "");
    let plan = load_plan(dir.path()).unwrap();
    assert!(plan.services.is_empty());
}

#[test]
fn ready_fields_are_validated() {
    let cases = [
        ("    ready: check\n", "needs ready-check"),
        (
            "    ready: check\n    ready-check: nope\n",
            "not a defined check",
        ),
        (
            "    ready: notify\n    ready-check: c\n",
            "only valid with ready: check",
        ),
        (
            "    ready-timeout: soon\n",
            "ready-timeout \"soon\" is not a duration",
        ),
    ];
    for (fields, want) in cases {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "001.yaml",
            &format!(
                "services:\n  app:\n    command: /bin/true\n{fields}\
                 checks:\n  c:\n    exec:\n      command: 'true'\n"
            ),
        );
        let err = format!("{:#}", load_plan(dir.path()).unwrap_err());
        assert!(err.contains(want), "{fields}: {err}");
    }
}

#[test]
fn ready_check_must_not_be_disabled() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001.yaml",
        "services:\n  app:\n    command: /bin/true\n    ready: check\n    ready-check: c\n\
         checks:\n  c:\n    startup: disabled\n    exec:\n      command: 'true'\n",
    );
    let err = format!("{:#}", load_plan(dir.path()).unwrap_err());
    assert!(err.contains("startup: disabled"), "{err}");
}

#[test]
fn valid_ready_config_loads() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001.yaml",
        "services:\n  app:\n    command: /bin/true\n    ready: check\n    ready-check: c\n    ready-timeout: 0s\n\
         \x20 web:\n    command: /bin/true\n    ready: notify\n\
         checks:\n  c:\n    startup: enabled\n    exec:\n      command: 'true'\n",
    );
    let plan = load_plan(dir.path()).unwrap();
    assert_eq!(
        plan.services["web"].ready,
        Some(vigil_types::plan::ReadyMode::Notify)
    );
}

#[test]
fn oneshot_fields_are_validated() {
    let cases = [
        (
            "    schedule: '* * * * *'\n",
            "schedule needs type: oneshot",
        ),
        (
            "    type: oneshot\n    schedule: 'nope'\n",
            "invalid schedule",
        ),
        (
            "    type: oneshot\n    schedule: '0 3 * * *'\n    schedule-tz: Mars/Olympus\n",
            "unknown schedule-tz",
        ),
        ("    schedule-tz: UTC\n", "only valid with schedule"),
        (
            "    type: oneshot\n    ready: notify\n",
            "do not apply to type: oneshot",
        ),
        (
            "    type: oneshot\n    on-success: restart\n",
            "on-success: restart",
        ),
    ];
    for (fields, want) in cases {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "001.yaml",
            &format!("services:\n  job:\n    command: /bin/true\n{fields}"),
        );
        let err = format!("{:#}", load_plan(dir.path()).unwrap_err());
        assert!(err.contains(want), "{fields}: {err}");
    }
}

#[test]
fn template_syntax_is_checked_at_load() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001.yaml",
        "services:\n  web:\n    command: app --port {{ env.PORT | }}\n",
    );
    let err = format!("{:#}", load_plan(dir.path()).unwrap_err());
    assert!(
        err.contains("service \"web\"") && err.contains("template in command"),
        "{err}"
    );

    write(
        &dir,
        "001.yaml",
        "services:\n  web:\n    command: app\n    env-file: [relative.env]\n",
    );
    let err = format!("{:#}", load_plan(dir.path()).unwrap_err());
    assert!(err.contains("path must be absolute"), "{err}");

    write(
        &dir,
        "001.yaml",
        "services:\n  web:\n    command: app\nchecks:\n  c:\n    exec:\n      command: 'true'\n      environment:\n        X: '{{ nope('\n",
    );
    let err = format!("{:#}", load_plan(dir.path()).unwrap_err());
    assert!(
        err.contains("check \"c\"") && err.contains("template in environment"),
        "{err}"
    );

    // valid templates load; undefined variables only fail at start
    write(
        &dir,
        "001.yaml",
        "services:\n  web:\n    command: app --port {{ env.PORT }}\n    environment:\n      PW: \"{{ file('/run/secrets/pw') }}\"\n",
    );
    load_plan(dir.path()).unwrap();
}

#[test]
fn alert_sources_are_validated() {
    let cases = [
        (
            "services:\n  s:\n    command: 'true'\n    healthy-time: 500ms\n",
            Some("must be between 1s and 1h"),
        ),
        (
            "services:\n  s:\n    command: 'true'\n    healthy-time: 2h\n",
            Some("must be between 1s and 1h"),
        ),
        (
            "services:\n  s:\n    command: 'true'\n    healthy-time: 1m\n",
            None,
        ),
        (
            "checks:\n  c:\n    service: nope\n    exec:\n      command: 'true'\n",
            Some("service \"nope\" is not defined"),
        ),
        (
            "services:\n  s:\n    command: 'true'\nchecks:\n  c:\n    service: s\n    exec:\n      command: 'true'\n",
            None,
        ),
        (
            "alerts:\n  a:\n    url: http://127.0.0.1/\n    on-vigild: [layer]\n",
            Some("unknown on-vigild entry \"layer\""),
        ),
        (
            "alerts:\n  a:\n    url: http://127.0.0.1/\n    on-vigild: [layers]\n    on-service: ['*']\n",
            None,
        ),
        (
            "alerts:\n  max-queue-depth: 0\n",
            Some("max-queue-depth must be at least 1"),
        ),
        (
            "alerts:\n  max-queue-time: 500ms\n",
            Some("must be at least 1s"),
        ),
        (
            "alerts:\n  max-queue-time: soon\n",
            Some("is not a duration"),
        ),
        (
            "alerts:\n  max-queue-depth: 1\n  max-queue-time: 1s\n",
            None,
        ),
    ];
    for (yaml, want) in cases {
        let dir = TempDir::new().unwrap();
        write(&dir, "001.yaml", yaml);
        match (load_plan(dir.path()), want) {
            (Ok(_), None) => {}
            (Err(e), Some(w)) => {
                let e = format!("{e:#}");
                assert!(e.contains(w), "{yaml}: {e}");
            }
            (r, w) => panic!("{yaml}: got {:?}, want {w:?}", r.map(|_| ())),
        }
    }
}

#[test]
fn invalid_values_are_rejected_with_the_field_name() {
    let svc = |f: &str| format!("services:\n  app:\n    command: /bin/true\n    {f}\n");
    let chk = |f: &str| format!("checks:\n  c:\n    {f}\n    exec:\n      command: 'true'\n");
    let alert = |f: &str| format!("alerts:\n  a:\n    url: http://127.0.0.1/\n    {f}\n");
    let cases = [
        (
            svc("kill-delay: 10 sec"),
            Some("kill-delay \"10 sec\" is not a duration"),
        ),
        (
            svc("backoff-delay: 0s"),
            Some("backoff-delay must be greater than 0"),
        ),
        (
            svc("backoff-delay: 1m"),
            Some("must not be shorter than backoff-delay"),
        ),
        (
            svc("backoff-factor: 0.5"),
            Some("backoff-factor 0.5 must be a number of at least 1.0"),
        ),
        (
            svc("backoff-limit: soon"),
            Some("backoff-limit \"soon\" is not a duration"),
        ),
        (svc("backoff-delay: 2s"), None),
        (chk("delay: 3x"), Some("delay \"3x\" is not a duration")),
        (chk("timeout: 0s"), Some("timeout must be greater than 0")),
        (chk("threshold: 0"), Some("threshold must be at least 1")),
        (
            alert("retry-attempts: 0"),
            Some("retry-attempts must be at least 1"),
        ),
        (
            alert("retry-backoff: [1s, later]"),
            Some("retry-backoff entry \"later\""),
        ),
        (alert("retry-attempts: 2"), None),
        (
            alert("body-template: '{{ unclosed'"),
            Some("body-template: syntax error"),
        ),
    ];
    for (yaml, want) in cases {
        let dir = TempDir::new().unwrap();
        write(&dir, "001.yaml", &yaml);
        match (load_plan(dir.path()), want) {
            (Ok(_), None) => {}
            (Err(e), Some(w)) => {
                let e = format!("{e:#}");
                assert!(e.contains(w), "{yaml}: {e}");
            }
            (r, w) => panic!("{yaml}: got {:?}, want {w:?}", r.map(|_| ())),
        }
    }
}

#[test]
fn check_period_is_bounded() {
    for (period, ok) in [
        ("0s", false),
        ("1ms", false),
        ("999ms", false),
        ("1s", true),
        ("24h", true),
        ("25h", false),
        ("soon", false),
    ] {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "001.yaml",
            &format!("checks:\n  c:\n    period: {period}\n    exec:\n      command: 'true'\n"),
        );
        match load_plan(dir.path()) {
            Ok(_) => assert!(ok, "{period} should be rejected"),
            Err(e) => {
                let e = format!("{e:#}");
                assert!(!ok, "{period}: {e}");
                assert!(e.contains("check \"c\""), "{e}");
            }
        }
    }
}

#[test]
fn watch_files_is_validated() {
    let cases = [
        ("    watch-files: {on-change: reload}\n", "expected restart"),
        (
            "    watch-files: {on-change: 'signal:SIGNOPE'}\n",
            "unknown signal",
        ),
        (
            "    watch-files: {interval: soon}\n",
            "watch-files interval \"soon\" is not a duration",
        ),
        ("    watch-files: {interval: 0s}\n", "between 1s and 24h"),
        ("    watch-files: {interval: 500ms}\n", "between 1s and 24h"),
        ("    watch-files: {interval: 25h}\n", "between 1s and 24h"),
        ("    watch-files: {paths: [rel.conf]}\n", "must be absolute"),
        ("    watch-files: {pahts: [/x]}\n", "unknown field"),
        (
            "    type: oneshot\n    watch-files: {on-change: restart}\n",
            "does not apply to type: oneshot",
        ),
    ];
    for (fields, want) in cases {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "001.yaml",
            &format!("services:\n  web:\n    command: /bin/true\n{fields}"),
        );
        let err = format!("{:#}", load_plan(dir.path()).unwrap_err());
        assert!(err.contains(want), "{fields}: {err}");
    }

    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001.yaml",
        "services:\n  web:\n    command: /bin/true\n    watch-files:\n      on-change: signal:SIGUSR1\n      interval: 10s\n      paths: [/etc/app.conf]\n",
    );
    let plan = load_plan(dir.path()).unwrap();
    let w = plan.services["web"].watch_files.clone().unwrap();
    assert_eq!(String::from(w.on_change.unwrap()), "signal:SIGUSR1");
}

#[test]
fn valid_oneshot_config_loads() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "001.yaml",
        "services:\n  job:\n    command: /bin/true\n    type: oneshot\n    schedule: '*/5 * * * *'\n    on-failure: restart\n",
    );
    let plan = load_plan(dir.path()).unwrap();
    assert_eq!(
        plan.services["job"].service_type,
        Some(vigil_types::plan::ServiceType::Oneshot)
    );
}
