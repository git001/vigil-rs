// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::extract::State;
use tokio::sync::oneshot;
use vigil_types::api::DaemonActionRequest;
use vigil_types::identity::IdentityAccess;

use crate::overlord::Cmd;

use super::super::{ApiError, ApiResult, AppState, auth::Caller, ok};
use super::{Body, json_body};

#[utoipa::path(
    post, path = "/v1/replan",
    tag = "replan",
    summary = "Reload layers and replan",
    description = "Reads all layer YAML files from the layers directory, re-merges them, starts newly enabled services, and stops removed ones.\n\n**Required access:** `write` or higher.",
    responses(
        (status = 200, description = "Replan completed successfully."),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(crate) async fn replan(caller: Caller, State(s): State<AppState>) -> ApiResult<()> {
    caller
        .require(IdentityAccess::Write)
        .map_err(ApiError::forbidden_from)?;
    let (tx, rx) = oneshot::channel();
    s.overlord.tx.send(Cmd::ReloadLayers { reply: tx }).await?;
    rx.await??;
    ok(())
}

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
    body: Body<DaemonActionRequest>,
) -> ApiResult<()> {
    caller
        .require(IdentityAccess::Admin)
        .map_err(ApiError::forbidden_from)?;
    let body = json_body(body)?;
    let _ = s.shutdown_tx.send(body.action).await;
    ok(())
}
