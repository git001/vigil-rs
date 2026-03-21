// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::extract::State;
use vigil_types::identity::IdentityAccess;

use super::super::{ApiError, AppState, auth::Caller};

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
pub(crate) async fn get_metrics(
    caller: Caller,
    State(s): State<AppState>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    caller
        .require(IdentityAccess::Metrics)
        .map_err(ApiError::forbidden_from)?;
    Ok((
        [(
            axum::http::header::CONTENT_TYPE,
            "application/openmetrics-text; version=1.0.0; charset=utf-8",
        )],
        s.metrics.render(),
    ))
}
