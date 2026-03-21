// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;

use axum::http::StatusCode;
use axum::{
    Json, Router,
    response::{Html, IntoResponse},
    routing::{get, post},
};
use utoipa::OpenApi;
use vigil_types::api::{
    AlertCheckStatus, AlertInfo, ChangeInfo, CheckInfo, DaemonAction, DaemonActionRequest,
    LogEntry, Response, ServiceInfo, ServicesAction, SystemInfo,
};
use vigil_types::identity::{
    AddIdentitiesRequest, Identity, IdentityAccess, IdentitySpec, LocalIdentity,
    RemoveIdentitiesRequest, TlsIdentity,
};
use vigil_types::plan::{CheckLevel, Startup};

use crate::identity::IdentityStore;
use crate::logs::LogStore;
use crate::metrics::MetricsStore;
use crate::overlord::Handle;

pub(super) mod auth;
mod handlers;

// ---------------------------------------------------------------------------
// OpenAPI spec
// ---------------------------------------------------------------------------

#[derive(OpenApi)]
#[openapi(
    info(
        title = "vigil API",
        version = "1",
        description = "HTTP API for the vigild service supervisor daemon.\n\nEndpoints are available over two transports:\n- **Unix socket** (default `/run/vigil/vigild.sock`) — local access only\n- **HTTPS** (optional, enabled via `--tls-addr`) — network access with TLS\n\n**Unix socket example:**\n```\ncurl --unix-socket /run/vigil/vigild.sock http://localhost/v1/system-info\n```\n\n**HTTPS example** (self-signed cert):\n```\ncurl --insecure https://localhost:8443/v1/system-info\n```"
    ),
    paths(
        handlers::system::system_info,
        handlers::services::list_services,
        handlers::services::services_action,
        handlers::services::get_change,
        handlers::checks::list_checks,
        handlers::alerts::list_alerts,
        handlers::logs::get_logs,
        handlers::logs::follow_logs,
        handlers::admin::replan,
        handlers::identities::list_identities,
        handlers::identities::add_identities,
        handlers::identities::remove_identities,
        handlers::metrics::get_metrics,
        handlers::identities::daemon_action,
    ),
    components(schemas(
        SystemInfo,
        ServiceInfo, ServicesAction,
        vigil_types::api::ServiceAction,
        vigil_types::api::ServiceStatus,
        vigil_types::api::ChangeStatus,
        ChangeInfo,
        CheckInfo,
        vigil_types::api::CheckStatus,
        AlertInfo, AlertCheckStatus,
        vigil_types::plan::AlertFormat,
        LogEntry,
        vigil_types::api::LogStream,
        Identity, IdentitySpec, IdentityAccess, LocalIdentity, TlsIdentity,
        AddIdentitiesRequest, RemoveIdentitiesRequest,
        Startup, CheckLevel,
        DaemonAction, DaemonActionRequest,
    )),
    tags(
        (name = "system info"),
        (name = "services"),
        (name = "changes"),
        (name = "checks"),
        (name = "alerts"),
        (name = "logs"),
        (name = "replan"),
        (name = "identities"),
        (name = "metrics"),
        (name = "daemon"),
    )
)]
pub struct ApiDoc;

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
}

// ---------------------------------------------------------------------------
// Error wrapper — supports 500, 403, 401
// ---------------------------------------------------------------------------

pub(super) struct ApiError(StatusCode, anyhow::Error);

impl ApiError {
    pub(super) fn forbidden() -> Self {
        ApiError(StatusCode::FORBIDDEN, anyhow::anyhow!("forbidden"))
    }

    /// Converts a `Caller::require` error into an `ApiError`.
    /// Signature matches `Result::map_err`'s closure expectation.
    pub(super) fn forbidden_from(_: (axum::http::StatusCode, &'static str)) -> Self {
        Self::forbidden()
    }

    pub(super) fn not_found(msg: impl Into<String>) -> Self {
        ApiError(StatusCode::NOT_FOUND, anyhow::anyhow!("{}", msg.into()))
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let code = self.0.as_u16();
        let status_text = self.0.canonical_reason().unwrap_or("Error");
        let body = Json(serde_json::json!({
            "type": "error",
            "status-code": code,
            "status": status_text,
            "result": null,
            "message": self.1.to_string()
        }));
        (self.0, body).into_response()
    }
}

pub(super) type ApiResult<T> = Result<Json<Response<T>>, ApiError>;

pub(super) fn ok<T>(val: T) -> ApiResult<T> {
    Ok(Json(Response {
        r#type: "sync".into(),
        status_code: 200,
        status: "OK".into(),
        result: Some(val),
        message: None,
    }))
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

async fn openapi_json() -> impl IntoResponse {
    Json(ApiDoc::openapi())
}

// NOTE: Swagger UI assets are loaded from unpkg.com (CDN). This works for
// development and internal networks with internet access. In air-gapped or
// restricted environments /docs will not render; use /openapi.json directly
// with a local Swagger/Redoc instance instead. Self-hosting the assets via
// the `utoipa-swagger-ui` crate would be the robust alternative if needed.
async fn swagger_ui() -> Html<&'static str> {
    Html(
        r#"<!DOCTYPE html>
<html>
<head>
  <title>vigil API</title>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <link rel="stylesheet" type="text/css" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css">
</head>
<body>
<div id="swagger-ui"></div>
<script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
<script>
  SwaggerUIBundle({
    url: "/openapi.json",
    dom_id: '#swagger-ui',
    presets: [SwaggerUIBundle.presets.apis, SwaggerUIBundle.SwaggerUIStandalonePreset],
    layout: "BaseLayout"
  })
</script>
</body>
</html>"#,
    )
}
