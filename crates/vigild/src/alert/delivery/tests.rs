// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use tokio::sync::mpsc;
use vigil_types::plan::{AlertConfig, AlertFormat, ProxyConfig, TlsConfig};

use super::prepare;
use super::send::http_send;
use super::worker::{DeliveryJob, delivery_worker};

fn proxy(url: &str, no_proxy: Option<&str>) -> Option<ProxyConfig> {
    Some(ProxyConfig {
        url: url.into(),
        no_proxy: no_proxy.map(Into::into),
    })
}

fn empty_cfg() -> AlertConfig {
    AlertConfig {
        url: "http://example.com".into(),
        format: Some(AlertFormat::Webhook),
        on_check: vec![],
        on_service: vec![],
        on_vigild: vec![],
        headers: IndexMap::new(),
        labels: IndexMap::new(),
        send_info_fields: IndexMap::new(),
        tls: None,
        proxy: None,
        retry_attempts: None,
        retry_backoff: vec![],
        override_mode: Default::default(),
        body_template: None,
        pass_env: None,
        pass_env_glob: vec![],
        pass_env_regex: vec![],
    }
}

// http_send: 2xx on the first attempt — no retry.
#[tokio::test]
async fn http_send_success_first_attempt() {
    let mock = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock)
        .await;
    let mut cfg = empty_cfg();
    cfg.url = mock.uri();
    http_send(&reqwest::Client::new(), &cfg, serde_json::json!({})).await;
    mock.verify().await;
}

// http_send: 5xx on all attempts → retries then gives up.
#[tokio::test]
async fn http_send_5xx_retries_then_gives_up() {
    let mock = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(503))
        .expect(3) // max_attempts defaults to 3
        .mount(&mock)
        .await;
    let mut cfg = empty_cfg();
    cfg.url = mock.uri();
    // Use zero backoff so the test runs fast
    cfg.retry_backoff = vec!["0ms".to_string(), "0ms".to_string()];
    http_send(&reqwest::Client::new(), &cfg, serde_json::json!({})).await;
    mock.verify().await;
}

// http_send: 5xx on first attempt, 2xx on second → success logged.
#[tokio::test]
async fn http_send_5xx_then_success_on_retry() {
    let mock = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&mock)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock)
        .await;
    let mut cfg = empty_cfg();
    cfg.url = mock.uri();
    cfg.retry_backoff = vec!["0ms".to_string()];
    http_send(&reqwest::Client::new(), &cfg, serde_json::json!({})).await;
    mock.verify().await;
}

// http_send: 4xx is NOT retried (client error).
#[tokio::test]
async fn http_send_4xx_not_retried() {
    let mock = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(400))
        .expect(1) // only once — no retry on 4xx
        .mount(&mock)
        .await;
    let mut cfg = empty_cfg();
    cfg.url = mock.uri();
    http_send(&reqwest::Client::new(), &cfg, serde_json::json!({})).await;
    mock.verify().await;
}

// delivery_worker: expired job (max_age_secs=0) is discarded without HTTP request.
#[tokio::test]
async fn delivery_worker_discards_expired_job() {
    use std::sync::atomic::AtomicU64;
    let mock = wiremock::MockServer::start().await;
    // Expect zero requests — the job should be discarded
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .expect(0)
        .mount(&mock)
        .await;

    let (tx, rx) = mpsc::channel(8);
    let max_age_secs = Arc::new(AtomicU64::new(0)); // 0s → always expired
    tokio::spawn(delivery_worker(rx, Arc::clone(&max_age_secs)));

    // A tiny sleep ensures elapsed() > 0 when the worker checks it
    tokio::time::sleep(Duration::from_millis(5)).await;
    let queued_at = tokio::time::Instant::now()
        .checked_sub(Duration::from_millis(10))
        .unwrap_or(tokio::time::Instant::now());

    let mut cfg = empty_cfg();
    cfg.url = mock.uri();
    tx.send(DeliveryJob {
        client: reqwest::Client::new(),
        config: cfg,
        body: serde_json::json!({}),
        queued_at,
    })
    .await
    .unwrap();
    drop(tx);
    tokio::time::sleep(Duration::from_millis(50)).await;
    mock.verify().await;
}

// http_send: connection error on all attempts → Err(e) branch retries then gives up.
#[tokio::test]
async fn http_send_connection_error_retries_then_gives_up() {
    let mut cfg = empty_cfg();
    // Port 1 is always closed → immediate connection refused.
    cfg.url = "http://127.0.0.1:1/alert".into();
    cfg.retry_backoff = vec!["0ms".to_string(), "0ms".to_string()];
    // Should complete without panic after 3 failed connection attempts.
    http_send(&reqwest::Client::new(), &cfg, serde_json::json!({})).await;
}

// http_send: invalid retry_backoff entry triggers the unwrap_or_else fallback to 1s.
// Use retry_attempts=2 so only one retry sleep occurs (≈1s total test time).
#[tokio::test]
async fn http_send_invalid_retry_backoff_falls_back_to_default() {
    let mock = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(503))
        .expect(2) // retry_attempts = 2 → two total requests
        .mount(&mock)
        .await;
    let mut cfg = empty_cfg();
    cfg.url = mock.uri();
    cfg.retry_attempts = Some(2);
    // "INVALID" is not a valid duration → parse_duration fails → unwrap_or_else → 1s sleep
    cfg.retry_backoff = vec!["INVALID".to_string()];
    http_send(&reqwest::Client::new(), &cfg, serde_json::json!({})).await;
    mock.verify().await;
}

// delivery_worker: fresh job (large max_age) is forwarded to http_send.
#[tokio::test]
async fn delivery_worker_sends_fresh_job() {
    use std::sync::atomic::AtomicU64;
    let mock = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock)
        .await;

    let (tx, rx) = mpsc::channel(8);
    let max_age_secs = Arc::new(AtomicU64::new(3600)); // 1 hour → never expires
    tokio::spawn(delivery_worker(rx, Arc::clone(&max_age_secs)));

    let mut cfg = empty_cfg();
    cfg.url = mock.uri();
    tx.send(DeliveryJob {
        client: reqwest::Client::new(),
        config: cfg,
        body: serde_json::json!({}),
        queued_at: tokio::time::Instant::now(),
    })
    .await
    .unwrap();
    drop(tx);
    tokio::time::sleep(Duration::from_millis(100)).await;
    mock.verify().await;
}

// ---------------------------------------------------------------------------
// prepare: env:VAR values and the HTTP client, checked when loading
// ---------------------------------------------------------------------------

/// vigild's environment for the test: just `pairs`.
fn env_of(pairs: &[(&str, &str)]) -> crate::alert::LoadEnv {
    crate::alert::LoadEnv {
        parent: pairs.iter().map(|(k, v)| (k.into(), v.into())).collect(),
    }
}

#[test]
fn prepare_resolves_every_env_field() {
    let mut cfg = empty_cfg();
    cfg.url = "env:ALERTMANAGER_URL".into();
    cfg.headers
        .insert("Authorization".into(), "env:ALERTMANAGER_AUTH".into());
    cfg.labels
        .insert("cluster".into(), "env:CLUSTER_NAME".into());
    cfg.labels.insert("env".into(), "production".into());
    cfg.send_info_fields
        .insert("k8s_namespace".into(), "env:KUBERNETES_NAMESPACE".into());
    cfg.proxy = proxy("env:HTTPS_PROXY", Some("env:NO_PROXY"));
    cfg.pass_env_glob = vec!["ALERTMANAGER_*".into(), "*_PROXY".into()];
    cfg.pass_env_regex = vec!["CLUSTER_NAME|KUBERNETES_.*".into()];
    let env = env_of(&[
        ("ALERTMANAGER_URL", "http://alertmanager:9093/api/v2/alerts"),
        ("ALERTMANAGER_AUTH", "Bearer eyJhbGciOiJIUzI1NiJ9"),
        ("CLUSTER_NAME", "prod-eu-1"),
        ("KUBERNETES_NAMESPACE", "shop"),
        ("HTTPS_PROXY", "http://proxy.corp:3128"),
        ("NO_PROXY", "alertmanager,.svc"),
    ]);
    let (out, _client) = prepare(&cfg, &env).unwrap();
    assert_eq!(out.url, "http://alertmanager:9093/api/v2/alerts");
    assert_eq!(out.headers["Authorization"], "Bearer eyJhbGciOiJIUzI1NiJ9");
    assert_eq!(out.labels["cluster"], "prod-eu-1");
    assert_eq!(out.labels["env"], "production", "literal values stay");
    assert_eq!(out.send_info_fields["k8s_namespace"], "shop");
    assert_eq!(
        out.proxy,
        proxy("http://proxy.corp:3128", Some("alertmanager,.svc"))
    );
}

#[test]
fn prepare_rejects_unset_or_empty_variables_naming_the_field() {
    for (setup, want) in [
        (
            (|c: &mut AlertConfig| c.url = "env:WEBHOOK_URL".into()) as fn(&mut AlertConfig),
            "url: environment variable WEBHOOK_URL is not set",
        ),
        (
            |c: &mut AlertConfig| {
                c.headers
                    .insert("Authorization".into(), "env:WEBHOOK_AUTH".into());
            },
            "headers.Authorization: environment variable WEBHOOK_AUTH is not set",
        ),
        (
            |c: &mut AlertConfig| {
                c.labels.insert("cluster".into(), "env:EMPTY".into());
            },
            "labels.cluster: environment variable EMPTY is not set",
        ),
        (
            |c: &mut AlertConfig| {
                c.send_info_fields.insert("team".into(), "env:TEAM".into());
            },
            "send-info-fields.team: environment variable TEAM",
        ),
        (
            |c: &mut AlertConfig| c.proxy = proxy("env:HTTPS_PROXY", None),
            "proxy.url: environment variable HTTPS_PROXY",
        ),
        (
            |c: &mut AlertConfig| {
                c.proxy = proxy("http://proxy.corp:3128", Some("env:NO_PROXY"));
            },
            "proxy.no-proxy: environment variable NO_PROXY",
        ),
    ] {
        let mut cfg = empty_cfg();
        setup(&mut cfg);
        cfg.pass_env = Some(true); // only "not set" is tested here
        let err = format!(
            "{:#}",
            prepare(&cfg, &env_of(&[("EMPTY", "")])).unwrap_err()
        );
        assert!(err.contains(want), "{err}");
    }
}

#[test]
fn prepare_only_sees_variables_the_policy_passes() {
    let env = env_of(&[
        ("WEBHOOK_URL", "http://hooks.example.com/alert"),
        ("DB_PASSWORD", "MY-pw1!"),
    ]);
    let mut cfg = empty_cfg();
    cfg.url = "env:WEBHOOK_URL".into();
    cfg.labels.insert("leak".into(), "env:DB_PASSWORD".into());
    // Default pass-env false: vigild's variables are not visible.
    let err = format!("{:#}", prepare(&cfg, &env).unwrap_err());
    assert!(
        err.contains(
            "url: environment variable WEBHOOK_URL is not passed to the alert; \
             allow it with pass-env-glob or pass-env-regex"
        ),
        "{err}"
    );
    assert!(!err.contains("MY-pw1!"), "never the value: {err}");

    cfg.pass_env_glob = vec!["WEBHOOK_*".into()];
    let err = format!("{:#}", prepare(&cfg, &env).unwrap_err());
    assert!(
        err.contains("labels.leak: environment variable DB_PASSWORD is not passed"),
        "{err}"
    );

    cfg.labels.clear();
    let (out, _) = prepare(&cfg, &env).unwrap();
    assert_eq!(out.url, "http://hooks.example.com/alert");

    // The base variables a service gets are visible without a list.
    let with_home = env_of(&[
        ("WEBHOOK_URL", "http://hooks.example.com/alert"),
        ("HOME", "/home/vigil"),
    ]);
    cfg.labels.insert("home".into(), "env:HOME".into());
    assert_eq!(
        prepare(&cfg, &with_home).unwrap().0.labels["home"],
        "/home/vigil"
    );
    cfg.labels.clear();

    // pass-env: true sees everything.
    cfg.pass_env_glob.clear();
    cfg.pass_env = Some(true);
    cfg.labels.insert("db".into(), "env:DB_PASSWORD".into());
    assert_eq!(prepare(&cfg, &env).unwrap().0.labels["db"], "MY-pw1!");
}

#[test]
fn proxy_variables_are_used_only_when_passed() {
    let env = env_of(&[
        ("HTTPS_PROXY", "http://proxy.corp:3128"),
        ("HTTP_PROXY", "http://other.corp:3128"),
        ("NO_PROXY", "alertmanager,.svc"),
    ]);
    let cfg = empty_cfg();
    let (out, _) = prepare(&cfg, &env).unwrap();
    assert_eq!(out.proxy, None, "not passed: no proxy");

    let mut cfg = empty_cfg();
    cfg.pass_env_glob = vec!["*_PROXY".into()];
    let (out, _) = prepare(&cfg, &env).unwrap();
    assert_eq!(
        out.proxy,
        proxy("http://proxy.corp:3128", Some("alertmanager,.svc")),
        "HTTPS_PROXY first, with NO_PROXY"
    );

    // A proxy: block wins over the variables, NO_PROXY included.
    cfg.proxy = proxy("http://explicit.corp:8080", None);
    let (out, _) = prepare(&cfg, &env).unwrap();
    assert_eq!(out.proxy, proxy("http://explicit.corp:8080", None));
}

#[test]
fn prepare_rejects_a_bad_proxy_or_tls_ca() {
    let mut cfg = empty_cfg();
    cfg.proxy = proxy("http://[::1", None);
    let err = format!("{:#}", prepare(&cfg, &env_of(&[])).unwrap_err());
    assert!(err.contains("proxy.url \"http://[::1\""), "{err}");

    let mut cfg = empty_cfg();
    cfg.tls = Some(TlsConfig {
        ca: vec!["/nonexistent/vigil-alert-ca.pem".into()],
        ..Default::default()
    });
    let err = format!("{:#}", prepare(&cfg, &env_of(&[])).unwrap_err());
    assert!(
        err.contains("HTTP client") && err.contains("tls.ca"),
        "{err}"
    );
}
