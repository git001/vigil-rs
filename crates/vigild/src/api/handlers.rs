// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::convert::Infallible;
use std::time::Duration;

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response, sse::{Event, KeepAlive, Sse}},
};
use serde::Deserialize;
use tokio::sync::oneshot;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use vigil_types::api::{
    ChangeInfo, CheckInfo, DaemonActionRequest, LogEntry, ServiceInfo, ServicesAction, SystemInfo,
};
use vigil_types::identity::{AddIdentitiesRequest, Identity, IdentityAccess, RemoveIdentitiesRequest};

use crate::overlord::Cmd;

use super::auth::Caller;
use super::{ok, ApiError, ApiResult, AppState};

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct NamesQuery {
    /// Comma-separated list of names to filter by. Omit for all.
    names: Option<String>,
}

fn parse_names(q: &NamesQuery) -> Vec<String> {
    q.names
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

#[derive(Deserialize, Default, Clone, Copy, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum LogFormat {
    /// JSON object per line: `{"timestamp":"…","service":"…","stream":"…","message":"…"}`
    #[default]
    Json,
    /// Plain text: `[service] message`
    Text,
    /// Newline-delimited JSON (`application/x-ndjson`): one JSON object per line, no SSE framing.
    /// Ideal for log collectors (Vector, Fluent Bit, …) that read from stdin.
    Ndjson,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct LogsQuery {
    /// Comma-separated list of service names to filter by. Omit for all services.
    services: Option<String>,
    /// Number of most-recent lines to return (default 100).
    n: Option<usize>,
    /// Response format for the follow stream: `json` (default) or `text`.
    format: Option<LogFormat>,
}

fn parse_services(q: &LogsQuery) -> Vec<String> {
    q.services
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

// ---------------------------------------------------------------------------
// Handlers — open (no auth required)
// ---------------------------------------------------------------------------

#[utoipa::path(
    get, path = "/v1/system-info",
    tag = "system info",
    summary = "Get system info",
    description = "Returns the daemon version, boot ID, start time, and API addresses.\n\n**Required access:** `open` (no authentication needed).",
    responses(
        (status = 200, description = "Daemon system information.", body = SystemInfo),
        (status = 500, description = "Internal error."),
    )
)]
pub(super) async fn system_info(State(s): State<AppState>) -> ApiResult<SystemInfo> {
    let (tx, rx) = oneshot::channel();
    s.overlord.tx.send(Cmd::GetSystemInfo { reply: tx }).await?;
    ok(rx.await?)
}

// ---------------------------------------------------------------------------
// Handlers — metrics level
// ---------------------------------------------------------------------------

/// Render Prometheus/OpenMetrics metrics.
#[utoipa::path(
    get, path = "/v1/metrics",
    tag = "metrics",
    description = "**Required access:** `metrics` or higher.",
    responses(
        (status = 200, description = "OpenMetrics text exposition.", content_type = "application/openmetrics-text; version=1.0.0; charset=utf-8"),
        (status = 403, description = "Forbidden."),
    )
)]
pub(super) async fn get_metrics(
    caller: Caller,
    State(s): State<AppState>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    caller.require(IdentityAccess::Metrics).map_err(ApiError::forbidden_from)?;
    Ok((
        [(
            axum::http::header::CONTENT_TYPE,
            "application/openmetrics-text; version=1.0.0; charset=utf-8",
        )],
        s.metrics.render(),
    ))
}

// ---------------------------------------------------------------------------
// Handlers — read level
// ---------------------------------------------------------------------------

#[utoipa::path(
    get, path = "/v1/services",
    tag = "services",
    summary = "List services",
    description = "Returns the current status of all or selected services.\n\n**Required access:** `read` or higher.",
    params(NamesQuery),
    responses(
        (status = 200, description = "List of service status entries.", body = Vec<ServiceInfo>),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(super) async fn list_services(
    caller: Caller,
    State(s): State<AppState>,
    Query(q): Query<NamesQuery>,
) -> ApiResult<Vec<ServiceInfo>> {
    caller.require(IdentityAccess::Read).map_err(ApiError::forbidden_from)?;
    let (tx, rx) = oneshot::channel();
    s.overlord.tx.send(Cmd::GetServices { names: parse_names(&q), reply: tx }).await?;
    ok(rx.await?)
}

#[utoipa::path(
    post, path = "/v1/services",
    tag = "services",
    summary = "Perform a service action",
    description = "Start, stop, restart, autostart, or replan one or more services.\n\nPass an empty `services` array to act on all services.\n\n**Required access:** `write` or higher.",
    request_body = ServicesAction,
    responses(
        (status = 200, description = "Change record for the performed action.", body = ChangeInfo),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(super) async fn services_action(
    caller: Caller,
    State(s): State<AppState>,
    Json(body): Json<ServicesAction>,
) -> ApiResult<ChangeInfo> {
    caller.require(IdentityAccess::Write).map_err(ApiError::forbidden_from)?;
    let (tx, rx) = oneshot::channel();
    s.overlord
        .tx
        .send(Cmd::Services { action: body.action, names: body.services, reply: tx })
        .await?;
    ok(rx.await??)
}

#[utoipa::path(
    get, path = "/v1/changes/{id}",
    tag = "changes",
    summary = "Get a change by ID",
    description = "**Required access:** `read` or higher.",
    params(("id" = String, Path, description = "Change ID")),
    responses(
        (status = 200, description = "The requested change record.", body = ChangeInfo),
        (status = 403, description = "Forbidden."),
        (status = 404, description = "Change not found."),
        (status = 500, description = "Internal error."),
    )
)]
pub(super) async fn get_change(
    caller: Caller,
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<ChangeInfo> {
    caller.require(IdentityAccess::Read).map_err(ApiError::forbidden_from)?;
    let (tx, rx) = oneshot::channel();
    s.overlord.tx.send(Cmd::GetChanges { id: Some(id.clone()), reply: tx }).await?;
    rx.await?
        .into_iter()
        .next()
        .map(ok)
        .unwrap_or_else(|| Err(ApiError::from(anyhow::anyhow!("change '{}' not found", id))))
}

#[utoipa::path(
    get, path = "/v1/checks",
    tag = "checks",
    summary = "List health checks",
    description = "Returns the current status of all or selected health checks.\n\n**Required access:** `read` or higher.",
    params(NamesQuery),
    responses(
        (status = 200, description = "List of check status entries.", body = Vec<CheckInfo>),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(super) async fn list_checks(
    caller: Caller,
    State(s): State<AppState>,
    Query(q): Query<NamesQuery>,
) -> ApiResult<Vec<CheckInfo>> {
    caller.require(IdentityAccess::Read).map_err(ApiError::forbidden_from)?;
    let (tx, rx) = oneshot::channel();
    s.overlord.tx.send(Cmd::GetChecks { names: parse_names(&q), reply: tx }).await?;
    ok(rx.await?)
}

#[utoipa::path(
    get, path = "/v1/logs",
    tag = "logs",
    summary = "Get recent log output",
    description = "Returns the most recent log lines from service stdout/stderr.\n\n**Required access:** `read` or higher.",
    params(LogsQuery),
    responses(
        (status = 200, description = "Recent log entries.", body = Vec<LogEntry>),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(super) async fn get_logs(
    caller: Caller,
    State(s): State<AppState>,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Vec<LogEntry>> {
    caller.require(IdentityAccess::Read).map_err(ApiError::forbidden_from)?;
    let n = q.n.unwrap_or(100);
    ok(s.log_store.tail(&parse_services(&q), n).await)
}

// ---------------------------------------------------------------------------
// Log follow (SSE) — read level
// ---------------------------------------------------------------------------

#[utoipa::path(
    get, path = "/v1/logs/follow",
    tag = "logs",
    summary = "Stream live log output",
    description = "Streams log entries in real time.\n\n**Formats** (`?format=`):\n- `json` *(default)* — SSE (`text/event-stream`), each event is `{\"timestamp\":\"…\",\"service\":\"…\",\"stream\":\"…\",\"message\":\"…\"}`. Keep-alive comments (`: ping`) every 15 s.\n- `text` — SSE, each event is `[service] message`.\n- `ndjson` — `application/x-ndjson`, one JSON object per line, no SSE framing. Best for log collectors.\n\n**Required access:** `read` or higher.",
    params(LogsQuery),
    responses(
        (status = 200, description = "Log stream (content-type depends on `format`).", content_type = "text/event-stream"),
        (status = 403, description = "Forbidden."),
    )
)]
pub(super) async fn follow_logs(
    caller: Caller,
    State(s): State<AppState>,
    Query(q): Query<LogsQuery>,
) -> Result<Response, ApiError> {
    caller.require(IdentityAccess::Read).map_err(ApiError::forbidden_from)?;
    let services = parse_services(&q);
    let format = q.format.unwrap_or_default();
    let rx = s.log_store.subscribe();

    if matches!(format, LogFormat::Ndjson) {
        // Plain newline-delimited JSON — no SSE framing, no keep-alives.
        // Each log entry is serialised as a JSON object followed by '\n'.
        let ndjson_stream = BroadcastStream::new(rx).filter_map(move |result| {
            match result {
                Ok(entry) if services.is_empty() || services.contains(&entry.service) => {
                    let mut line = serde_json::to_string(&entry).unwrap_or_default();
                    line.push('\n');
                    Some(Ok::<_, Infallible>(line))
                }
                _ => None,
            }
        });
        let response = Response::builder()
            .header(header::CONTENT_TYPE, "application/x-ndjson")
            .body(Body::from_stream(ndjson_stream))
            .unwrap();
        return Ok(response);
    }

    // SSE path (json or text).
    let sse_stream = BroadcastStream::new(rx).filter_map(move |result| {
        match result {
            Ok(entry) if services.is_empty() || services.contains(&entry.service) => {
                let data = match format {
                    LogFormat::Json | LogFormat::Ndjson => {
                        serde_json::to_string(&entry).unwrap_or_default()
                    }
                    LogFormat::Text => format!("[{}] {}", entry.service, entry.message),
                };
                Some(Ok::<_, Infallible>(Event::default().data(data)))
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                Some(Ok::<_, Infallible>(Event::default().comment(format!("lagged: skipped {} entries", n))))
            }
            _ => None,
        }
    });

    Ok(Sse::new(sse_stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("ping"),
        )
        .into_response())
}

// ---------------------------------------------------------------------------
// Handlers — write level
// ---------------------------------------------------------------------------

#[utoipa::path(
    post, path = "/v1/replan",
    tag = "replan",
    summary = "Reload layers and replan",
    description = "Reads all layer YAML files from the layers directory, re-merges them, starts newly enabled services, and stops removed ones.\n\n**Required access:** `write` or higher.",
    responses(
        (status = 200, description = "Replan completed successfully."),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(super) async fn replan(caller: Caller, State(s): State<AppState>) -> ApiResult<()> {
    caller.require(IdentityAccess::Write).map_err(ApiError::forbidden_from)?;
    let (tx, rx) = oneshot::channel();
    s.overlord.tx.send(Cmd::ReloadLayers { reply: tx }).await?;
    rx.await??;
    ok(())
}

// ---------------------------------------------------------------------------
// Handlers — admin level
// ---------------------------------------------------------------------------

#[utoipa::path(
    post, path = "/v1/vigild",
    tag = "daemon",
    summary = "Control the daemon",
    description = "Stop or restart the vigild daemon gracefully.\n\n`stop` stops all services and exits. `restart` stops all services and re-executes the daemon in-place.\n\n**Required access:** `admin`.",
    request_body = DaemonActionRequest,
    responses(
        (status = 200, description = "Action accepted."),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(super) async fn daemon_action(
    caller: Caller,
    State(s): State<AppState>,
    Json(body): Json<DaemonActionRequest>,
) -> ApiResult<()> {
    caller.require(IdentityAccess::Admin).map_err(ApiError::forbidden_from)?;
    let _ = s.shutdown_tx.send(body.action).await;
    ok(())
}

#[utoipa::path(
    get, path = "/v1/identities",
    tag = "identities",
    summary = "List identities",
    description = "**Required access:** `admin`.",
    params(NamesQuery),
    responses(
        (status = 200, description = "List of identities.", body = Vec<Identity>),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(super) async fn list_identities(
    caller: Caller,
    State(s): State<AppState>,
    Query(q): Query<NamesQuery>,
) -> ApiResult<Vec<Identity>> {
    caller.require(IdentityAccess::Admin).map_err(ApiError::forbidden_from)?;
    ok(s.identity_store.list(&parse_names(&q)).await)
}

#[utoipa::path(
    post, path = "/v1/identities",
    tag = "identities",
    summary = "Add or update identities",
    description = "**Required access:** `admin`.",
    request_body = AddIdentitiesRequest,
    responses(
        (status = 200, description = "Identities added or updated."),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(super) async fn add_identities(
    caller: Caller,
    State(s): State<AppState>,
    Json(body): Json<AddIdentitiesRequest>,
) -> ApiResult<()> {
    caller.require(IdentityAccess::Admin).map_err(ApiError::forbidden_from)?;
    for (name, spec) in body.identities {
        s.identity_store.add(name, spec).await;
    }
    ok(())
}

#[utoipa::path(
    delete, path = "/v1/identities",
    tag = "identities",
    summary = "Remove identities",
    description = "**Required access:** `admin`.",
    request_body = RemoveIdentitiesRequest,
    responses(
        (status = 200, description = "Names of the removed identities.", body = Vec<String>),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(super) async fn remove_identities(
    caller: Caller,
    State(s): State<AppState>,
    Json(body): Json<RemoveIdentitiesRequest>,
) -> ApiResult<Vec<String>> {
    caller.require(IdentityAccess::Admin).map_err(ApiError::forbidden_from)?;
    ok(s.identity_store.remove(&body.identities).await)
}
