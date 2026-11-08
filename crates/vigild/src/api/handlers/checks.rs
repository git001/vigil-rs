// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::extract::{Query, State};
use tokio::sync::oneshot;
use vigil_types::api::{CheckInfo, CheckRefresh, CheckRefreshRequest};
use vigil_types::identity::IdentityAccess;

use crate::overlord::Cmd;

use super::super::{ApiError, ApiResult, AppState, auth::Caller, ok};
use super::{Body, NamesQuery, json_body, parse_names};

#[utoipa::path(
    get, path = "/v1/checks",
    tag = "checks",
    summary = "List health checks",
    description = "Returns the current status of all or selected health checks.\n\n**Required access:** `open` (no credentials; usable by Kubernetes httpGet probes).",
    params(NamesQuery),
    responses(
        (status = 200, description = "List of check status entries.", body = Vec<CheckInfo>),
        (status = 500, description = "Internal error."),
    )
)]
pub(crate) async fn list_checks(
    caller: Caller,
    State(s): State<AppState>,
    Query(q): Query<NamesQuery>,
) -> ApiResult<Vec<CheckInfo>> {
    // Open: only names and up/down state, needed by unauthenticated
    // Kubernetes probes (httpGet cannot send credentials).
    caller
        .require(IdentityAccess::Open)
        .map_err(ApiError::forbidden_from)?;
    let (tx, rx) = oneshot::channel();
    s.overlord
        .tx
        .send(Cmd::GetChecks {
            names: parse_names(&q),
            reply: tx,
        })
        .await?;
    ok(rx.await?)
}

#[utoipa::path(
    post, path = "/v1/checks/refresh",
    tag = "checks",
    summary = "Run a check now",
    description = "Runs the check at once and returns its state after the run, plus `error`: why this run failed (absent when it passed).\n\nThe run counts like a scheduled one: failures, threshold, status and alerts. A check with `startup: disabled` runs once without changing any state.\n\n**Required access:** `write` or higher.",
    request_body = CheckRefreshRequest,
    responses(
        (status = 200, description = "The check after the run.", body = CheckRefresh),
        (status = 403, description = "Forbidden."),
        (status = 404, description = "No check of that name."),
        (status = 500, description = "Internal error."),
    )
)]
pub(crate) async fn refresh_check(
    caller: Caller,
    State(s): State<AppState>,
    body: Body<CheckRefreshRequest>,
) -> ApiResult<CheckRefresh> {
    caller
        .require(IdentityAccess::Write)
        .map_err(ApiError::forbidden_from)?;
    let body = json_body(body)?;
    let (tx, rx) = oneshot::channel();
    s.overlord
        .tx
        .send(Cmd::RefreshCheck {
            name: body.name.clone(),
            reply: tx,
        })
        .await?;
    match rx.await? {
        Some(result) => ok(result),
        None => Err(ApiError::not_found(format!(
            "check {:?} not found",
            body.name
        ))),
    }
}
