// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::extract::State;
use tokio::sync::oneshot;
use vigil_types::identity::IdentityAccess;
use vigil_types::plan::Layer;

use crate::overlord::Cmd;

use super::super::{ApiError, ApiResult, AppState, auth::Caller, mask::mask, ok};

#[utoipa::path(
    get, path = "/v1/plan",
    tag = "plan",
    summary = "Get the plan",
    description = "Returns the plan merged from all layers, in the form of one layer file (`services`, `checks`, `alerts`, `identities`; the same keys as in the layer files). Read as a layer file, it gives the same plan.\n\nSecrets are masked as `***`: every `environment` value; `headers` values other than a reference `env:NAME`; in URLs the user info and the query values, and for alerts also the path (webhooks carry their secret there); `password-hash` of identities. `command`, `body-template`, `labels`, `send-info-fields` and file paths are shown as written.\n\n**Required access:** `read` or higher.",
    responses(
        (status = 200, description = "The merged plan as one layer, secrets masked.", body = Object),
        (status = 403, description = "Forbidden."),
        (status = 500, description = "Internal error."),
    )
)]
pub(crate) async fn get_plan(caller: Caller, State(s): State<AppState>) -> ApiResult<Layer> {
    caller
        .require(IdentityAccess::Read)
        .map_err(ApiError::forbidden_from)?;
    let (tx, rx) = oneshot::channel();
    s.overlord.tx.send(Cmd::GetPlan { reply: tx }).await?;
    let mut layer = rx.await?;
    mask(&mut layer);
    ok(layer)
}
