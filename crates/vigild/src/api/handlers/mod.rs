// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

pub(super) mod admin;
pub(super) mod alerts;
pub(super) mod checks;
pub(super) mod identities;
pub(super) mod logs;
pub(super) mod metrics;
pub(super) mod services;
pub(super) mod system;

// ---------------------------------------------------------------------------
// Query helpers — shared across submodules
// ---------------------------------------------------------------------------

use serde::Deserialize;

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct NamesQuery {
    /// Comma-separated list of names to filter by. Omit for all.
    names: Option<String>,
}

pub(super) fn parse_names(q: &NamesQuery) -> Vec<String> {
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

pub(super) fn parse_services(q: &LogsQuery) -> Vec<String> {
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
// Re-exports — keep all handler names accessible as `handlers::<name>`
// ---------------------------------------------------------------------------

pub(crate) use admin::replan;
pub(crate) use alerts::list_alerts;
pub(crate) use checks::list_checks;
pub(crate) use identities::{add_identities, daemon_action, list_identities, remove_identities};
pub(crate) use logs::{follow_logs, get_logs};
pub(crate) use metrics::get_metrics;
pub(crate) use services::{get_change, list_services, services_action};
pub(crate) use system::system_info;
