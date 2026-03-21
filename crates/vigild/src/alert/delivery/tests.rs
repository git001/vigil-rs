// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use tokio::sync::mpsc;
use vigil_types::plan::{AlertConfig, AlertFormat};

use super::send::http_send;
use super::warn_unset_env_vars;
use super::worker::{DeliveryJob, delivery_worker};

fn empty_cfg() -> AlertConfig {
    AlertConfig {
        url: "http://example.com".into(),
        format: AlertFormat::Webhook,
        on_check: vec![],
        headers: IndexMap::new(),
        labels: IndexMap::new(),
        send_info_fields: IndexMap::new(),
        tls_insecure: false,
        tls_ca: None,
        proxy: None,
        proxy_ca: None,
        no_proxy: None,
        retry_attempts: None,
        retry_backoff: vec![],
        override_mode: Default::default(),
        body_template: None,
    }
}

#[test]
fn warn_unset_env_vars_no_panic() {
    let mut cfg = empty_cfg();
    cfg.url = "env:_VIGIL_DEFINITELY_MISSING_9999".into();
    cfg.labels.insert(
        "cluster".into(),
        "env:_VIGIL_DEFINITELY_MISSING_9999".into(),
    );
    warn_unset_env_vars("test-alert", &cfg);
}

#[test]
fn warn_unset_env_vars_set_var_no_warning() {
    unsafe {
        std::env::set_var("_VIGIL_WARN_TEST", "present");
    }
    let mut cfg = empty_cfg();
    cfg.url = "env:_VIGIL_WARN_TEST".into();
    warn_unset_env_vars("test-alert", &cfg);
    unsafe {
        std::env::remove_var("_VIGIL_WARN_TEST");
    }
}

// http_send: empty URL (unset env var) returns without making any HTTP request.
#[tokio::test]
async fn http_send_empty_url_returns_early() {
    // The env var is not set → resolve() returns "" → early return
    let mut cfg = empty_cfg();
    cfg.url = "env:_VIGIL_DEFINITELY_MISSING_URL_9998".into();
    // Should return without panic and without hitting any real server
    http_send(&reqwest::Client::new(), &cfg, serde_json::json!({"x": 1})).await;
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

// warn_unset_env_vars: proxy and no_proxy fields with env: prefix
#[test]
fn warn_unset_env_vars_proxy_and_no_proxy() {
    let mut cfg = empty_cfg();
    cfg.proxy = Some("env:_VIGIL_DEFINITELY_MISSING_9999".into());
    cfg.no_proxy = Some("env:_VIGIL_DEFINITELY_MISSING_9999".into());
    warn_unset_env_vars("test-alert", &cfg); // must not panic
}

// warn_unset_env_vars: headers and send_info_fields with env: prefix
#[test]
fn warn_unset_env_vars_headers_and_send_info_fields() {
    let mut cfg = empty_cfg();
    cfg.headers.insert(
        "X-Token".into(),
        "env:_VIGIL_DEFINITELY_MISSING_9999".into(),
    );
    cfg.send_info_fields
        .insert("team".into(), "env:_VIGIL_DEFINITELY_MISSING_9999".into());
    warn_unset_env_vars("test-alert", &cfg); // must not panic
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
