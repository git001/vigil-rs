// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::convert::Infallible;
use std::time::Duration;

use axum::{
    body::Body,
    extract::State,
    http::header,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use vigil_types::api::LogEntry;
use vigil_types::identity::IdentityAccess;

use super::super::{ApiError, AppState, auth::Caller, ok};
use super::{Params, query_params, split_list};

/// Lines `GET /v1/logs` returns when `n` is not given.
const DEFAULT_TAIL: usize = 100;

#[derive(Deserialize, Default, Clone, Copy, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LogFormat {
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
#[serde(deny_unknown_fields)]
pub(crate) struct LogsQuery {
    /// Comma-separated list of service names to filter by. Omit for all services.
    services: Option<String>,
    /// Number of most recent lines to return (default 100). With
    /// `follow=true` these come first, then the new lines.
    n: Option<usize>,
    /// `true` keeps the connection open and streams newline-delimited JSON
    /// (`application/x-ndjson`), like `/v1/logs/follow?format=ndjson`.
    follow: Option<bool>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub(crate) struct FollowQuery {
    /// Comma-separated list of service names to filter by. Omit for all services.
    services: Option<String>,
    /// Stream format: `json` (default), `text` or `ndjson`.
    format: Option<LogFormat>,
}

#[utoipa::path(
    get, path = "/v1/logs",
    tag = "logs",
    summary = "Get recent log output",
    description = "Returns the most recent log lines from service stdout/stderr.\n\nWith `follow=true` the response is a stream instead: the last `n` lines, then each new line, as newline-delimited JSON (`application/x-ndjson`), until the client closes the connection. An unknown query parameter is `400`.\n\n**Required access:** `read` or higher.",
    params(LogsQuery),
    responses(
        (status = 200, description = "Recent log entries; with `follow=true` an `application/x-ndjson` stream.", body = Vec<LogEntry>),
        (status = 400, description = "Unknown query parameter or invalid value."),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(crate) async fn get_logs(
    caller: Caller,
    State(s): State<AppState>,
    q: Params<LogsQuery>,
) -> Result<Response, ApiError> {
    caller
        .require(IdentityAccess::Read)
        .map_err(ApiError::forbidden_from)?;
    let q = query_params(q)?;
    let services = split_list(q.services.as_deref());
    let n = q.n.unwrap_or(DEFAULT_TAIL);
    if q.follow.unwrap_or(false) {
        // Subscribe before reading the tail, so no line falls between them.
        let rx = s.log_store.subscribe();
        let recent = s.log_store.tail(&services, n).await;
        return Ok(ndjson_response(recent, rx, services));
    }
    Ok(ok(s.log_store.tail(&services, n).await)?.into_response())
}

/// Streams `recent`, then the live entries of `services` (all when empty),
/// one JSON object per line. A live entry that is already in `recent`
/// (pushed between `subscribe` and `tail`) is not sent twice.
fn ndjson_response(
    recent: Vec<LogEntry>,
    rx: broadcast::Receiver<LogEntry>,
    services: Vec<String>,
) -> Response {
    let last_sent = recent.last().map(|e| e.timestamp);
    let sent = recent.clone();
    let live = BroadcastStream::new(rx).filter_map(move |result| match result {
        Ok(entry)
            if wanted(&services, &entry)
                && !(last_sent.is_some_and(|t| entry.timestamp <= t) && sent.contains(&entry)) =>
        {
            Some(entry)
        }
        _ => None,
    });
    let lines = tokio_stream::iter(recent).chain(live).map(|entry| {
        let mut line = serde_json::to_string(&entry).unwrap_or_default();
        line.push('\n');
        Ok::<_, Infallible>(line)
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(lines))
        .unwrap()
}

fn wanted(services: &[String], entry: &LogEntry) -> bool {
    services.is_empty() || services.contains(&entry.service)
}

#[utoipa::path(
    get, path = "/v1/logs/follow",
    tag = "logs",
    summary = "Stream live log output",
    description = "Streams log entries in real time.\n\n**Formats** (`?format=`):\n- `json` *(default)* — SSE (`text/event-stream`), each event is `{\"timestamp\":\"…\",\"service\":\"…\",\"stream\":\"…\",\"message\":\"…\"}`. Keep-alive comments (`: ping`) every 15 s.\n- `text` — SSE, each event is `[service] message`.\n- `ndjson` — `application/x-ndjson`, one JSON object per line, no SSE framing. Best for log collectors.\n\nAn unknown query parameter or an invalid `format` is `400`.\n\n**Required access:** `read` or higher.",
    params(FollowQuery),
    responses(
        (status = 200, description = "Log stream (content-type depends on `format`).", content_type = "text/event-stream"),
        (status = 400, description = "Unknown query parameter or invalid value."),
        (status = 403, description = "Forbidden."),
    )
)]
pub(crate) async fn follow_logs(
    caller: Caller,
    State(s): State<AppState>,
    q: Params<FollowQuery>,
) -> Result<Response, ApiError> {
    caller
        .require(IdentityAccess::Read)
        .map_err(ApiError::forbidden_from)?;
    let q = query_params(q)?;
    let services = split_list(q.services.as_deref());
    let format = q.format.unwrap_or_default();
    let rx = s.log_store.subscribe();

    if matches!(format, LogFormat::Ndjson) {
        // Plain newline-delimited JSON: no SSE framing, no keep-alives.
        return Ok(ndjson_response(Vec::new(), rx, services));
    }

    // SSE path (json or text).
    let sse_stream = BroadcastStream::new(rx).filter_map(move |result| match result {
        Ok(entry) if wanted(&services, &entry) => {
            let data = match format {
                LogFormat::Json | LogFormat::Ndjson => {
                    serde_json::to_string(&entry).unwrap_or_default()
                }
                LogFormat::Text => format!("[{}] {}", entry.service, entry.message),
            };
            Some(Ok::<_, Infallible>(Event::default().data(data)))
        }
        Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
            Some(Ok::<_, Infallible>(
                Event::default().comment(format!("lagged: skipped {} entries", n)),
            ))
        }
        _ => None,
    });

    Ok(Sse::new(sse_stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("ping"),
        )
        .into_response())
}
