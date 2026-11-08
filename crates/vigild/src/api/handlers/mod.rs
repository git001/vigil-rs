// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

pub(super) mod admin;
pub(super) mod alerts;
pub(super) mod checks;
pub(super) mod health;
pub(super) mod identities;
pub(super) mod logs;
pub(super) mod metrics;
pub(super) mod services;
pub(super) mod signals;
pub(super) mod system;

// ---------------------------------------------------------------------------
// Query helpers — shared across submodules
// ---------------------------------------------------------------------------

use axum::Json;
use axum::extract::Query;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use serde::Deserialize;

use super::ApiError;

/// A JSON request body, taken as a `Result` so the handler can check the
/// caller's access first: a caller without access learns nothing about the
/// body. Unwrap it with [`json_body`].
pub(super) type Body<T> = Result<Json<T>, JsonRejection>;

/// The body of [`Body`], or its rejection as an error envelope with the
/// rejection's status (415 wrong content type, 400 invalid JSON, 422 unknown
/// field or wrong type).
pub(super) fn json_body<T>(body: Body<T>) -> Result<T, ApiError> {
    body.map(|Json(b)| b)
        .map_err(|e| ApiError::with_status(e.status(), anyhow::anyhow!(e.body_text())))
}

/// Query parameters, taken as a `Result` for the same reason as [`Body`].
/// Unwrap them with [`query_params`].
pub(super) type Params<T> = Result<Query<T>, QueryRejection>;

/// The parameters of [`Params`], or `400` as an error envelope (unknown
/// parameter or invalid value).
pub(super) fn query_params<T>(query: Params<T>) -> Result<T, ApiError> {
    query
        .map(|Query(q)| q)
        .map_err(|e| ApiError::bad_request(anyhow::anyhow!(e.body_text())))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub(super) struct NamesQuery {
    /// Comma-separated list of names to filter by. Omit for all.
    names: Option<String>,
}

pub(super) fn parse_names(q: &NamesQuery) -> Vec<String> {
    split_list(q.names.as_deref())
}

/// Splits a comma-separated query value (`services=web,worker`) into names;
/// empty when the parameter is missing.
pub(super) fn split_list(value: Option<&str>) -> Vec<String> {
    value
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

pub(crate) use admin::{daemon_action, replan};
pub(crate) use alerts::list_alerts;
pub(crate) use checks::{list_checks, refresh_check};
pub(crate) use health::health;
pub(crate) use identities::{add_identities, list_identities, remove_identities};
pub(crate) use logs::{follow_logs, get_logs};
pub(crate) use metrics::get_metrics;
pub(crate) use services::{get_change, list_services, services_action};
pub(crate) use signals::send_signals;
pub(crate) use system::system_info;
