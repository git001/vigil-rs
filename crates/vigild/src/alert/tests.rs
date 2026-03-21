// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::atomic::Ordering;
use std::time::Duration;

use indexmap::IndexMap;
use vigil_types::api::CheckStatus;
use vigil_types::plan::{AlertConfig, AlertFormat};

use super::delivery::DEFAULT_DELIVERY_AGE;
use super::{AlertEntry, AlertSender};

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

fn make_sender(check: &str) -> AlertSender {
    let mut cfg = empty_cfg();
    cfg.on_check = vec![check.to_owned()];
    let mut sender = AlertSender::new();
    sender.alerts = vec![AlertEntry {
        name: check.to_owned(),
        config: cfg,
        client: reqwest::Client::new(),
    }];
    sender
}

// -----------------------------------------------------------------------
// AlertSender dedup logic
// -----------------------------------------------------------------------

#[tokio::test]
async fn first_up_suppressed() {
    let mut s = make_sender("web");
    s.handle_check_event("web", CheckStatus::Up);
    assert_eq!(s.check_status("web"), Some(CheckStatus::Up));
}

#[tokio::test]
async fn first_down_is_not_suppressed() {
    let mut s = make_sender("web");
    s.handle_check_event("web", CheckStatus::Down);
    assert_eq!(s.check_status("web"), Some(CheckStatus::Down));
}

#[tokio::test]
async fn duplicate_status_suppressed() {
    let mut s = make_sender("web");
    s.handle_check_event("web", CheckStatus::Down);
    s.handle_check_event("web", CheckStatus::Down);
    assert_eq!(s.check_status("web"), Some(CheckStatus::Down));
}

#[tokio::test]
async fn down_then_up_transition() {
    let mut s = make_sender("web");
    s.handle_check_event("web", CheckStatus::Down);
    s.handle_check_event("web", CheckStatus::Up);
    assert_eq!(s.check_status("web"), Some(CheckStatus::Up));
}

#[tokio::test]
async fn unknown_check_returns_none() {
    let s = AlertSender::new();
    assert_eq!(s.check_status("nonexistent"), None);
}

#[tokio::test]
async fn alert_only_fires_for_subscribed_check() {
    let mut s = make_sender("db");
    s.handle_check_event("web", CheckStatus::Down);
    assert_eq!(s.check_status("web"), Some(CheckStatus::Down));
    assert_eq!(s.check_status("db"), None);
}

// -----------------------------------------------------------------------
// update_queue_limits
// -----------------------------------------------------------------------

#[test]
fn update_queue_limits_changes_max_age() {
    let mut s = AlertSender::with_queue_limits(64, Duration::from_secs(30));
    assert_eq!(s.max_age_secs.load(Ordering::Relaxed), 30);
    s.update_queue_limits(64, Duration::from_secs(120));
    assert_eq!(s.max_age_secs.load(Ordering::Relaxed), 120);
    assert_eq!(s.queue_depth, 64);
}

#[tokio::test]
async fn update_queue_limits_replaces_channel_on_depth_change() {
    let mut s = AlertSender::with_queue_limits(64, Duration::from_secs(30));
    s.spawn_worker();
    s.update_queue_limits(128, Duration::from_secs(30));
    assert_eq!(s.queue_depth, 128);
    assert!(s.delivery_tx.capacity() > 0);
}

#[test]
fn update_queue_limits_same_depth_no_channel_replace() {
    let mut s = AlertSender::with_queue_limits(256, Duration::from_secs(60));
    s.update_queue_limits(256, Duration::from_secs(10));
    assert_eq!(s.queue_depth, 256);
    assert_eq!(s.max_age_secs.load(Ordering::Relaxed), 10);
}

// -----------------------------------------------------------------------
// defaults
// -----------------------------------------------------------------------

#[test]
fn default_delivery_age_is_60s() {
    assert_eq!(DEFAULT_DELIVERY_AGE.as_secs(), 60);
}
