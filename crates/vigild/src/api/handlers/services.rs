// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::extract::{Path, State};
use tokio::sync::oneshot;
use vigil_types::api::{ChangeInfo, ServiceInfo, ServicesAction};
use vigil_types::identity::IdentityAccess;

use crate::overlord::Cmd;

use super::super::{ApiError, ApiResult, AppState, auth::Caller, ok};
use super::{Body, NamesQuery, Params, json_body, parse_names, query_params};

#[utoipa::path(
    get, path = "/v1/services",
    tag = "services",
    summary = "List services",
    description = "Returns the current status of all or selected services.\n\n**Required access:** `read` or higher.",
    params(NamesQuery),
    responses(
        (status = 200, description = "List of service status entries.", body = Vec<ServiceInfo>),
        (status = 400, description = "Unknown query parameter."),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(crate) async fn list_services(
    caller: Caller,
    State(s): State<AppState>,
    q: Params<NamesQuery>,
) -> ApiResult<Vec<ServiceInfo>> {
    caller
        .require(IdentityAccess::Read)
        .map_err(ApiError::forbidden_from)?;
    let q = query_params(q)?;
    let (tx, rx) = oneshot::channel();
    s.overlord
        .tx
        .send(Cmd::GetServices {
            names: parse_names(&q),
            reply: tx,
        })
        .await?;
    ok(rx.await?)
}

#[utoipa::path(
    post, path = "/v1/services",
    tag = "services",
    summary = "Perform a service action",
    description = "Start, stop, restart, autostart, or replan one or more services.\n\nPass an empty `services` array to act on all services; for `autostart`, all services with `startup: enabled` that are not scheduled. `replan` reloads the layers once, like `POST /v1/replan`, and ignores `services`.\n\nThe answer comes when the action is done: `start`, `restart` and `autostart` once the services are ready (a oneshot once its run has ended), `stop` once their processes are gone. Meanwhile `GET /v1/changes/{id}` shows the change as `doing`.\n\n**Required access:** `write` or higher.",
    request_body = ServicesAction,
    responses(
        (status = 200, description = "Change record for the performed action.", body = ChangeInfo),
        (status = 400, description = "Invalid JSON."),
        (status = 403, description = "Forbidden."),
        (status = 415, description = "Content-Type is not `application/json`."),
        (status = 422, description = "Unknown field or wrong type in the body."),
        (status = 500, description = "Internal error."),
    )
)]
pub(crate) async fn services_action(
    caller: Caller,
    State(s): State<AppState>,
    body: Body<ServicesAction>,
) -> ApiResult<ChangeInfo> {
    caller
        .require(IdentityAccess::Write)
        .map_err(ApiError::forbidden_from)?;
    let body = json_body(body)?;
    let (tx, rx) = oneshot::channel();
    s.overlord
        .tx
        .send(Cmd::Services {
            action: body.action,
            names: body.services,
            reply: tx,
        })
        .await?;
    ok(rx.await??)
}

#[utoipa::path(
    get, path = "/v1/changes/{id}",
    tag = "changes",
    summary = "Get a change by ID",
    description = "**Required access:** `read` or higher.",
    params(("id" = String, Path, description = "Change ID")),
    responses(
        (status = 200, description = "The requested change record.", body = ChangeInfo),
        (status = 403, description = "Forbidden."),
        (status = 404, description = "Change not found."),
        (status = 500, description = "Internal error."),
    )
)]
pub(crate) async fn get_change(
    caller: Caller,
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<ChangeInfo> {
    caller
        .require(IdentityAccess::Read)
        .map_err(ApiError::forbidden_from)?;
    let (tx, rx) = oneshot::channel();
    s.overlord
        .tx
        .send(Cmd::GetChanges {
            id: Some(id.clone()),
            reply: tx,
        })
        .await?;
    rx.await?
        .into_iter()
        .next()
        .map(ok)
        .unwrap_or_else(|| Err(ApiError::not_found(format!("change '{}' not found", id))))
}
