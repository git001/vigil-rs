// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::extract::State;
use tokio::sync::oneshot;
use vigil_types::api::AlertInfo;
use vigil_types::identity::IdentityAccess;

use crate::overlord::Cmd;

use super::super::{ApiError, ApiResult, AppState, auth::Caller, ok};
use super::{NamesQuery, Params, parse_names, query_params};

#[utoipa::path(
    get, path = "/v1/alerts",
    tag = "alerts",
    summary = "List alert configurations",
    description = "Returns all configured alerts with their format, watched checks, and last observed check status per check.\n\n**Required access:** `read` or higher.",
    params(NamesQuery),
    responses(
        (status = 200, description = "List of alert entries.", body = Vec<AlertInfo>),
        (status = 400, description = "Unknown query parameter."),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(crate) async fn list_alerts(
    caller: Caller,
    State(s): State<AppState>,
    q: Params<NamesQuery>,
) -> ApiResult<Vec<AlertInfo>> {
    caller
        .require(IdentityAccess::Read)
        .map_err(ApiError::forbidden_from)?;
    let q = query_params(q)?;
    let (tx, rx) = oneshot::channel();
    s.overlord
        .tx
        .send(Cmd::GetAlerts {
            names: parse_names(&q),
            reply: tx,
        })
        .await?;
    ok(rx.await?)
}
