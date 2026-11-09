// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response as HttpResponse};
use std::sync::atomic::Ordering;

use serde::Deserialize;
use tokio::sync::oneshot;
use vigil_types::api::{HealthInfo, Response};
use vigil_types::identity::IdentityAccess;
use vigil_types::plan::CheckLevel;

use crate::overlord::Cmd;

use super::super::{ApiError, AppState, auth::Caller};
use super::{Params, query_params, split_list};

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub(crate) struct HealthQuery {
    /// `alive`: only checks with `level: alive`. `ready`: checks of both
    /// levels. Omit for all checks.
    level: Option<CheckLevel>,
    /// Comma-separated check names. Omit for all checks.
    names: Option<String>,
}

#[utoipa::path(
    get, path = "/v1/health",
    tag = "checks",
    summary = "Health of the checks",
    description = "vigild's health endpoint, for Kubernetes `httpGet` probes: `200` with `{\"healthy\": true}` when none of the selected checks is down, otherwise `502` with `{\"healthy\": false}`.\n\nOnly running checks count; a check with `startup: disabled` is ignored. `level=ready` includes `alive` checks. A name that is not a check is `404`, an unknown query parameter `400`.\n\nWhile vigild waits out `--shutdown-delay` after SIGTERM, the answer is `503` with `{\"healthy\": false}` for `level=ready` and without `level`, so the pod leaves the load balancer while its services still run; `level=alive` keeps reporting the checks.\n\n**Required access:** `open` (no credentials; usable by Kubernetes httpGet probes).",
    params(HealthQuery),
    responses(
        (status = 200, description = "No selected check is down.", body = HealthInfo),
        (status = 400, description = "Unknown query parameter or level."),
        (status = 404, description = "A name is not a check."),
        (status = 502, description = "At least one selected check is down.", body = HealthInfo),
        (status = 503, description = "vigild is shutting down (`--shutdown-delay`); not for `level=alive`.", body = HealthInfo),
    )
)]
pub(crate) async fn health(
    caller: Caller,
    State(s): State<AppState>,
    q: Params<HealthQuery>,
) -> Result<HttpResponse, ApiError> {
    caller
        .require(IdentityAccess::Open)
        .map_err(ApiError::forbidden_from)?;
    let q = query_params(q)?;
    // Shutting down: not ready any more, whatever the checks say. The
    // liveness probe (level=alive) still gets the checks, so the kubelet
    // does not restart a container that is about to stop anyway.
    if q.level != Some(CheckLevel::Alive) && s.stopping.load(Ordering::Relaxed) {
        let code = StatusCode::SERVICE_UNAVAILABLE;
        let body = Response {
            r#type: "sync".into(),
            status_code: code.as_u16(),
            status: code.canonical_reason().unwrap_or_default().into(),
            result: Some(HealthInfo { healthy: false }),
            message: Some("vigild is shutting down".into()),
        };
        return Ok((code, Json(body)).into_response());
    }
    let names = split_list(q.names.as_deref());
    let (tx, rx) = oneshot::channel();
    s.overlord
        .tx
        .send(Cmd::Health {
            level: q.level,
            names,
            reply: tx,
        })
        .await?;
    let healthy = rx.await?.map_err(ApiError::not_found)?;
    // 502 Bad Gateway: vigild answers, what it watches does not.
    let code = if healthy {
        StatusCode::OK
    } else {
        StatusCode::BAD_GATEWAY
    };
    let body = Response {
        r#type: "sync".into(),
        status_code: code.as_u16(),
        status: code.canonical_reason().unwrap_or_default().into(),
        result: Some(HealthInfo { healthy }),
        message: None,
    };
    Ok((code, Json(body)).into_response())
}
