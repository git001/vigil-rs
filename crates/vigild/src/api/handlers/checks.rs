// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::extract::{Query, State};
use tokio::sync::oneshot;
use vigil_types::api::CheckInfo;
use vigil_types::identity::IdentityAccess;

use crate::overlord::Cmd;

use super::{NamesQuery, parse_names};
use super::super::{ApiError, ApiResult, AppState, auth::Caller, ok};

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
pub(crate) async fn list_checks(
    caller: Caller,
    State(s): State<AppState>,
    Query(q): Query<NamesQuery>,
) -> ApiResult<Vec<CheckInfo>> {
    caller
        .require(IdentityAccess::Read)
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
