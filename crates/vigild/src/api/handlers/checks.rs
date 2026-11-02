// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::extract::{Query, State};
use tokio::sync::oneshot;
use vigil_types::api::CheckInfo;
use vigil_types::identity::IdentityAccess;

use crate::overlord::Cmd;

use super::super::{ApiError, ApiResult, AppState, auth::Caller, ok};
use super::{NamesQuery, parse_names};

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
