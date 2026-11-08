// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::extract::State;
use vigil_types::identity::{
    AddIdentitiesRequest, Identity, IdentityAccess, RemoveIdentitiesRequest,
};

use super::super::{ApiError, ApiResult, AppState, auth::Caller, ok};
use super::{Body, NamesQuery, Params, json_body, parse_names, query_params};

#[utoipa::path(
    get, path = "/v1/identities",
    tag = "identities",
    summary = "List identities",
    description = "**Required access:** `admin`.",
    params(NamesQuery),
    responses(
        (status = 200, description = "List of identities.", body = Vec<Identity>),
        (status = 400, description = "Unknown query parameter."),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(crate) async fn list_identities(
    caller: Caller,
    State(s): State<AppState>,
    q: Params<NamesQuery>,
) -> ApiResult<Vec<Identity>> {
    caller
        .require(IdentityAccess::Admin)
        .map_err(ApiError::forbidden_from)?;
    let q = query_params(q)?;
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
        (status = 409, description = "A name belongs to an identity from a layer file (read-only over the API)."),
        (status = 500, description = "Internal error."),
    )
)]
pub(crate) async fn add_identities(
    caller: Caller,
    State(s): State<AppState>,
    body: Body<AddIdentitiesRequest>,
) -> ApiResult<()> {
    caller
        .require(IdentityAccess::Admin)
        .map_err(ApiError::forbidden_from)?;
    let body = json_body(body)?;
    for (name, spec) in body.identities {
        s.identity_store
            .add(name, spec)
            .await
            .map_err(ApiError::conflict)?;
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
        (status = 409, description = "A name belongs to an identity from a layer file; nothing was removed."),
        (status = 500, description = "Internal error."),
    )
)]
pub(crate) async fn remove_identities(
    caller: Caller,
    State(s): State<AppState>,
    body: Body<RemoveIdentitiesRequest>,
) -> ApiResult<Vec<String>> {
    caller
        .require(IdentityAccess::Admin)
        .map_err(ApiError::forbidden_from)?;
    let body = json_body(body)?;
    s.identity_store
        .remove(&body.identities)
        .await
        .map_err(ApiError::conflict)
        .and_then(ok)
}
