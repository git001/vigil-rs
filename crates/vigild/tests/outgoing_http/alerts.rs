// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use super::*;

// ---------------------------------------------------------------------------
// Alert webhook delivery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn alert_down_fires_webhook_post() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let mut sender = sender_with_url(&mock_server.uri());
    sender.handle_check_event("my-check", CheckStatus::Down);

    wait_for_requests(&mock_server, 1, Duration::from_millis(500)).await;
    mock_server.verify().await;
}

#[tokio::test]
async fn alert_first_up_does_not_fire_webhook() {
    let mock_server = MockServer::start().await;

    // Expect zero calls — first Up with no prior Down must be suppressed.
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&mock_server)
        .await;

    let mut sender = sender_with_url(&mock_server.uri());
    sender.handle_check_event("my-check", CheckStatus::Up);

    // Brief wait to confirm nothing arrives.
    tokio::time::sleep(Duration::from_millis(50)).await;
    mock_server.verify().await;
}

#[tokio::test]
async fn alert_recovery_fires_after_down() {
    let mock_server = MockServer::start().await;

    // Two calls expected: Down then Up.
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(2)
        .mount(&mock_server)
        .await;

    let mut sender = sender_with_url(&mock_server.uri());
    sender.handle_check_event("my-check", CheckStatus::Down);
    sender.handle_check_event("my-check", CheckStatus::Up);

    wait_for_requests(&mock_server, 2, Duration::from_millis(500)).await;
    mock_server.verify().await;
}

#[tokio::test]
async fn alert_duplicate_status_does_not_fire_twice() {
    let mock_server = MockServer::start().await;

    // Only one call expected: second Down is a duplicate and must be suppressed.
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let mut sender = sender_with_url(&mock_server.uri());
    sender.handle_check_event("my-check", CheckStatus::Down);
    sender.handle_check_event("my-check", CheckStatus::Down);

    wait_for_requests(&mock_server, 1, Duration::from_millis(500)).await;
    mock_server.verify().await;
}

#[tokio::test]
async fn alert_webhook_body_contains_check_name() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/hook"))
        .and(body_string_contains("my-check"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let mut sender = sender_with_url(&mock_server.uri());
    sender.handle_check_event("my-check", CheckStatus::Down);

    wait_for_requests(&mock_server, 1, Duration::from_millis(500)).await;
    mock_server.verify().await;
}

#[tokio::test]
async fn alert_webhook_body_indicates_down_status() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/hook"))
        .and(body_string_contains("down"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let mut sender = sender_with_url(&mock_server.uri());
    sender.handle_check_event("my-check", CheckStatus::Down);

    wait_for_requests(&mock_server, 1, Duration::from_millis(500)).await;
    mock_server.verify().await;
}

#[tokio::test]
async fn alert_custom_header_is_sent() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/hook"))
        .and(header("X-Token", "secret"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let mut sender = AlertSender::new();
    let mut alerts = IndexMap::new();
    let mut cfg = alert_cfg(&mock_server.uri());
    cfg.headers
        .insert("X-Token".to_string(), "secret".to_string());
    alerts.insert("my-alert".to_string(), cfg);
    sender.update_alerts(alerts);
    sender.spawn_worker();
    sender.handle_check_event("my-check", CheckStatus::Down);

    wait_for_requests(&mock_server, 1, Duration::from_millis(500)).await;
    mock_server.verify().await;
}

#[tokio::test]
async fn alert_retries_on_server_error_and_eventually_succeeds() {
    let mock_server = MockServer::start().await;

    // First response: 500 (triggers retry). Second: 200 (success).
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let mut sender = sender_with_url(&mock_server.uri());
    sender.handle_check_event("my-check", CheckStatus::Down);

    // Default backoff: 1s before first retry — wait long enough for retry.
    wait_for_requests(&mock_server, 2, Duration::from_millis(3000)).await;
    let reqs = mock_server.received_requests().await.unwrap();
    assert!(reqs.len() >= 2, "expected ≥2 requests, got {}", reqs.len());
}

// ---------------------------------------------------------------------------
// No matching check — alert must NOT fire
// ---------------------------------------------------------------------------

#[tokio::test]
async fn alert_not_triggered_for_unrelated_check() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&mock_server)
        .await;

    let mut sender = sender_with_url(&mock_server.uri());
    // "other-check" is not in the alert's on_check list
    sender.handle_check_event("other-check", CheckStatus::Down);

    // Brief wait to confirm nothing arrives.
    tokio::time::sleep(Duration::from_millis(50)).await;
    mock_server.verify().await;
}
