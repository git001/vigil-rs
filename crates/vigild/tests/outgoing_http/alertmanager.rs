// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use super::*;

// ---------------------------------------------------------------------------
// Alertmanager format
// ---------------------------------------------------------------------------

#[tokio::test]
async fn alertmanager_format_fires_on_down() {
    let mock_server = MockServer::start().await;

    // Alertmanager format signals active alert by setting endsAt to zero epoch
    // and startsAt to the event time — no literal "firing" text in the body.
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let mut sender = AlertSender::new();
    let mut alerts = IndexMap::new();
    alerts.insert(
        "am-alert".to_string(),
        AlertConfig {
            url: format!("{}/hook", mock_server.uri()),
            format: AlertFormat::Alertmanager,
            on_check: vec!["my-check".to_string()],
            ..Default::default()
        },
    );
    sender.update_alerts(alerts);
    sender.spawn_worker();
    sender.handle_check_event("my-check", CheckStatus::Down);

    wait_for_requests(&mock_server, 1, Duration::from_millis(500)).await;
    mock_server.verify().await;
}

#[tokio::test]
async fn alertmanager_format_fires_resolved_on_up() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let mut sender = AlertSender::new();
    let mut alerts = IndexMap::new();
    alerts.insert(
        "am-alert".to_string(),
        AlertConfig {
            url: format!("{}/hook", mock_server.uri()),
            format: AlertFormat::Alertmanager,
            on_check: vec!["my-check".to_string()],
            ..Default::default()
        },
    );
    sender.update_alerts(alerts);
    sender.spawn_worker();

    sender.handle_check_event("my-check", CheckStatus::Down);
    sender.handle_check_event("my-check", CheckStatus::Up);

    wait_for_requests(&mock_server, 2, Duration::from_millis(500)).await;
    let reqs = mock_server.received_requests().await.unwrap();
    assert_eq!(
        reqs.len(),
        2,
        "expected exactly 2 requests (down + recovery)"
    );

    // Alertmanager recovery: endsAt is set to an actual timestamp,
    // startsAt uses the zero epoch (0001-01-01T00:00:00Z).
    let recovery_body = std::str::from_utf8(&reqs[1].body).unwrap();
    assert!(
        recovery_body.contains("0001-01-01T00:00:00Z"),
        "recovery body should have zero-epoch startsAt: {recovery_body}"
    );
}
