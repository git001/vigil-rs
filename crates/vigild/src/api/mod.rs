// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The HTTP API: state shared by the handlers and the router.

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use vigil_types::api::DaemonAction;
use vigil_types::identity::IdentityAccess;

use crate::identity::IdentityStore;
use crate::logs::LogStore;
use crate::metrics::MetricsStore;
use crate::overlord::Handle;

pub(super) mod auth;
mod error;
mod handlers;
mod openapi;

pub(crate) use error::{ApiError, ApiResult, ok};
pub use openapi::ApiDoc;
use openapi::{openapi_json, swagger_ui};

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub overlord: Handle,
    pub log_store: Arc<LogStore>,
    pub identity_store: Arc<IdentityStore>,
    pub metrics: Arc<MetricsStore>,
    pub shutdown_tx: tokio::sync::mpsc::Sender<DaemonAction>,
    /// Access level of TLS callers without a matching identity
    /// (`--tls-anonymous-access`, `Open` or `Metrics`).
    pub tls_anonymous_access: IdentityAccess,
    /// vigild is shutting down (`--shutdown-delay`): `/v1/health` reports
    /// not ready while the services still run.
    pub stopping: Arc<std::sync::atomic::AtomicBool>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/system-info", get(handlers::system_info))
        .route("/v1/metrics", get(handlers::get_metrics))
        .route(
            "/v1/services",
            get(handlers::list_services).post(handlers::services_action),
        )
        .route("/v1/changes/{id}", get(handlers::get_change))
        .route("/v1/checks", get(handlers::list_checks))
        .route("/v1/checks/refresh", post(handlers::refresh_check))
        .route("/v1/health", get(handlers::health))
        .route("/v1/signals", post(handlers::send_signals))
        .route("/v1/alerts", get(handlers::list_alerts))
        .route("/v1/logs", get(handlers::get_logs))
        .route("/v1/logs/follow", get(handlers::follow_logs))
        .route("/v1/replan", post(handlers::replan))
        .route("/v1/vigild", post(handlers::daemon_action))
        .route(
            "/v1/identities",
            get(handlers::list_identities)
                .post(handlers::add_identities)
                .delete(handlers::remove_identities),
        )
        .route("/docs", get(swagger_ui))
        .route("/openapi.json", get(openapi_json))
        .with_state(state)
}
