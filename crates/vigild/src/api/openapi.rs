// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The OpenAPI document, `/openapi.json` and the Swagger UI at `/docs`.

use axum::Json;
use axum::response::{Html, IntoResponse};
use utoipa::OpenApi;
use vigil_types::api::{
    AlertCheckStatus, AlertInfo, ChangeInfo, CheckInfo, DaemonAction, DaemonActionRequest,
    LogEntry, ServiceInfo, ServicesAction, SystemInfo,
};
use vigil_types::identity::{
    AddIdentitiesRequest, Identity, IdentityAccess, IdentitySpec, LocalIdentity,
    RemoveIdentitiesRequest, TlsIdentity,
};
use vigil_types::plan::{CheckLevel, Startup};

use super::handlers;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "vigil API",
        version = "4",
        description = "HTTP API for the vigild service supervisor daemon.\n\nEndpoints are available over two transports:\n- **Unix socket** (default `/run/vigil/vigild.sock`) — local access only\n- **HTTPS** (optional, enabled via `--tls-addr`) — network access with TLS\n\n**Unix socket example:**\n```\ncurl --unix-socket /run/vigil/vigild.sock http://localhost/v1/system-info\n```\n\n**HTTPS example** (self-signed cert):\n```\ncurl --insecure https://localhost:8443/v1/system-info\n```"
    ),
    paths(
        handlers::system::system_info,
        handlers::services::list_services,
        handlers::services::services_action,
        handlers::services::get_change,
        handlers::checks::list_checks,
        handlers::checks::refresh_check,
        handlers::health::health,
        handlers::signals::send_signals,
        handlers::alerts::list_alerts,
        handlers::logs::get_logs,
        handlers::logs::follow_logs,
        handlers::plan::get_plan,
        handlers::admin::replan,
        handlers::identities::list_identities,
        handlers::identities::add_identities,
        handlers::identities::remove_identities,
        handlers::metrics::get_metrics,
        handlers::admin::daemon_action,
    ),
    components(schemas(
        SystemInfo,
        ServiceInfo, ServicesAction,
        vigil_types::api::ServiceAction,
        vigil_types::api::ServiceStatus,
        vigil_types::api::ExitInfo,
        vigil_types::api::ChangeStatus,
        ChangeInfo,
        CheckInfo,
        vigil_types::api::CheckRefreshRequest,
        vigil_types::api::CheckRefresh,
        vigil_types::api::HealthInfo,
        vigil_types::api::SignalsRequest,
        vigil_types::api::SignalsResult,
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
        (name = "plan"),
        (name = "replan"),
        (name = "identities"),
        (name = "metrics"),
        (name = "daemon"),
    )
)]
pub struct ApiDoc;

#[cfg(test)]
mod tests;

pub(super) async fn openapi_json() -> impl IntoResponse {
    Json(ApiDoc::openapi())
}

// NOTE: Swagger UI assets are loaded from unpkg.com (CDN). This works for
// development and internal networks with internet access. In air-gapped or
// restricted environments /docs will not render; use /openapi.json directly
// with a local Swagger/Redoc instance instead. Self-hosting the assets via
// the `utoipa-swagger-ui` crate would be the robust alternative if needed.
pub(super) async fn swagger_ui() -> Html<&'static str> {
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
