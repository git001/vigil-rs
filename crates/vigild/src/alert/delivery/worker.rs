// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use reqwest::Client;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::warn;
use vigil_types::plan::AlertConfig;

use super::send::http_send;

pub(crate) struct DeliveryJob {
    pub(crate) client: Client,
    pub(crate) config: AlertConfig,
    pub(crate) body: Value,
    /// Wall-clock instant at which this job was enqueued.
    pub(crate) queued_at: tokio::time::Instant,
}

/// Background task: drains the delivery channel, spawning one Tokio task per
/// job so that concurrent alerts never block each other.
/// Jobs older than `max_age_secs` (read atomically on each job) are discarded
/// without delivery, allowing `max-queue-time` to take effect after a replan.
pub(crate) async fn delivery_worker(
    mut rx: mpsc::Receiver<DeliveryJob>,
    max_age_secs: Arc<AtomicU64>,
) {
    while let Some(job) = rx.recv().await {
        let max_age = Duration::from_secs(max_age_secs.load(Ordering::Relaxed));
        let age = job.queued_at.elapsed();
        if age > max_age {
            warn!(
                age_secs = age.as_secs(),
                max_secs = max_age.as_secs(),
                "alert delivery job expired in queue — discarding"
            );
            continue;
        }
        tokio::spawn(async move {
            http_send(&job.client, &job.config, job.body).await;
        });
    }
}
