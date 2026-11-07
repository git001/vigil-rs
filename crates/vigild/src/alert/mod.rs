// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Alert routing — fires HTTP(S) notifications for events of checks,
//! services and vigild itself (see [`AlertEvent`]).
//!
//! Supported formats: webhook (generic JSON), Alertmanager, CloudEvents 1.0,
//! OTLP HTTP/JSON logs.
//!
//! Values of `url`, `headers`, `labels`, `send-info-fields`, `proxy` and
//! `no-proxy` that start with `"env:"` come from vigild's environment, so you
//! can inject runtime context without rebuilding the image. They are
//! resolved when the layers are loaded; an unset variable makes the layer
//! invalid:
//!
//! ```yaml
//! alerts:
//!   prod:
//!     url: http://alertmanager:9093/api/v2/alerts
//!     format: alertmanager
//!     on-check: [website]
//!     on-service: ["*"]
//!     labels:
//!       cluster: "env:CLUSTER_NAME"
//!     send-info-fields:
//!       k8s_service: "env:KUBERNETES_SERVICE_NAME"
//! ```

mod delivery;
mod event;
mod format;
mod prepare;
mod sender;
#[cfg(test)]
mod tests;

pub use self::delivery::{
    DEFAULT_DELIVERY_AGE, DEFAULT_DELIVERY_QUEUE, queue_limits, validate_queue_limits,
};
pub use self::event::{AlertEvent, AlertSource};
#[cfg(test)]
use self::prepare::AlertEntry;
pub use self::prepare::{LoadEnv, PreparedAlerts, prepare_alerts};
pub use self::sender::AlertSender;
