// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::{Json, extract::{Query, State}};
use vigil_types::api::DaemonActionRequest;
use vigil_types::identity::{
    AddIdentitiesRequest, Identity, IdentityAccess, RemoveIdentitiesRequest,
};

use super::{NamesQuery, parse_names};
use super::super::{ApiError, ApiResult, AppState, auth::Caller, ok};

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
pub(crate) async fn daemon_action(
    caller: Caller,
    State(s): State<AppState>,
    Json(body): Json<DaemonActionRequest>,
) -> ApiResult<()> {
    caller
        .require(IdentityAccess::Admin)
        .map_err(ApiError::forbidden_from)?;
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
pub(crate) async fn list_identities(
    caller: Caller,
    State(s): State<AppState>,
    Query(q): Query<NamesQuery>,
) -> ApiResult<Vec<Identity>> {
    caller
        .require(IdentityAccess::Admin)
        .map_err(ApiError::forbidden_from)?;
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
pub(crate) async fn add_identities(
    caller: Caller,
    State(s): State<AppState>,
    Json(body): Json<AddIdentitiesRequest>,
) -> ApiResult<()> {
    caller
        .require(IdentityAccess::Admin)
        .map_err(ApiError::forbidden_from)?;
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
pub(crate) async fn remove_identities(
    caller: Caller,
    State(s): State<AppState>,
    Json(body): Json<RemoveIdentitiesRequest>,
) -> ApiResult<Vec<String>> {
    caller
        .require(IdentityAccess::Admin)
        .map_err(ApiError::forbidden_from)?;
    ok(s.identity_store.remove(&body.identities).await)
}
