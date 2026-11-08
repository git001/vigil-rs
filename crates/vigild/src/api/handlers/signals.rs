// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::str::FromStr;

use axum::extract::State;
use axum::http::StatusCode;
use nix::sys::signal::Signal;
use tokio::sync::oneshot;
use vigil_types::api::{SignalsRequest, SignalsResult};
use vigil_types::identity::IdentityAccess;

use crate::overlord::{Cmd, SignalError};

use super::super::{ApiError, ApiResult, AppState, auth::Caller, ok};
use super::{Body, json_body};

#[utoipa::path(
    post, path = "/v1/signals",
    tag = "services",
    summary = "Send a signal to services",
    description = "Sends `signal` to the process group of each service in `services`, e.g. `SIGHUP` to make nginx reload its configuration. Every service must exist and have a running process (`starting`, `active` or `stopping`); otherwise none gets the signal. Signal names are written as in `stop-signal` (`SIGHUP`, `SIGUSR1`).\n\n**Required access:** `write` or higher.",
    request_body = SignalsRequest,
    responses(
        (status = 200, description = "The services that got the signal.", body = SignalsResult),
        (status = 400, description = "Unknown signal, empty `services` or unknown field."),
        (status = 403, description = "Forbidden."),
        (status = 404, description = "A name is not a service."),
        (status = 409, description = "A service has no running process."),
    )
)]
pub(crate) async fn send_signals(
    caller: Caller,
    State(s): State<AppState>,
    body: Body<SignalsRequest>,
) -> ApiResult<SignalsResult> {
    caller
        .require(IdentityAccess::Write)
        .map_err(ApiError::forbidden_from)?;
    let body = json_body(body)?;
    let signal = Signal::from_str(&body.signal).map_err(|_| {
        ApiError::bad_request(anyhow::anyhow!(
            "unknown signal {:?} (write it like SIGHUP)",
            body.signal
        ))
    })?;
    let (tx, rx) = oneshot::channel();
    s.overlord
        .tx
        .send(Cmd::Signal {
            signal,
            services: body.services,
            reply: tx,
        })
        .await?;
    match rx.await? {
        Ok(services) => ok(SignalsResult {
            signal: signal.to_string(),
            services,
        }),
        Err(e) => {
            let code = match e {
                SignalError::NoServices => StatusCode::BAD_REQUEST,
                SignalError::NotFound(_) => StatusCode::NOT_FOUND,
                SignalError::NotRunning(_) => StatusCode::CONFLICT,
            };
            Err(ApiError::with_status(code, anyhow::anyhow!("{e}")))
        }
    }
}
