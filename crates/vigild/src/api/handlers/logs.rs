// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::convert::Infallible;
use std::time::Duration;

use axum::{
    body::Body,
    extract::{Query, State},
    http::header,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use vigil_types::api::LogEntry;
use vigil_types::identity::IdentityAccess;

use super::super::{ApiError, ApiResult, AppState, auth::Caller, ok};
use super::{LogFormat, LogsQuery, parse_services};

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
pub(crate) async fn get_logs(
    caller: Caller,
    State(s): State<AppState>,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Vec<LogEntry>> {
    caller
        .require(IdentityAccess::Read)
        .map_err(ApiError::forbidden_from)?;
    let n = q.n.unwrap_or(100);
    ok(s.log_store.tail(&parse_services(&q), n).await)
}

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
pub(crate) async fn follow_logs(
    caller: Caller,
    State(s): State<AppState>,
    Query(q): Query<LogsQuery>,
) -> Result<Response, ApiError> {
    caller
        .require(IdentityAccess::Read)
        .map_err(ApiError::forbidden_from)?;
    let services = parse_services(&q);
    let format = q.format.unwrap_or_default();
    let rx = s.log_store.subscribe();

    if matches!(format, LogFormat::Ndjson) {
        // Plain newline-delimited JSON — no SSE framing, no keep-alives.
        // Each log entry is serialised as a JSON object followed by '\n'.
        let ndjson_stream = BroadcastStream::new(rx).filter_map(move |result| match result {
            Ok(entry) if services.is_empty() || services.contains(&entry.service) => {
                let mut line = serde_json::to_string(&entry).unwrap_or_default();
                line.push('\n');
                Some(Ok::<_, Infallible>(line))
            }
            _ => None,
        });
        let response = Response::builder()
            .header(header::CONTENT_TYPE, "application/x-ndjson")
            .body(Body::from_stream(ndjson_stream))
            .unwrap();
        return Ok(response);
    }

    // SSE path (json or text).
    let sse_stream = BroadcastStream::new(rx).filter_map(move |result| match result {
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
