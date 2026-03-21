// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::extract::State;
use tokio::sync::oneshot;
use vigil_types::api::SystemInfo;

use crate::overlord::Cmd;

use super::super::{ApiResult, AppState, ok};

#[utoipa::path(
    get, path = "/v1/system-info",
    tag = "system info",
    summary = "Get system info",
    description = "Returns the daemon version, boot ID, start time, and API addresses.\n\n**Required access:** `open` (no authentication needed).",
    responses(
        (status = 200, description = "Daemon system information.", body = SystemInfo),
        (status = 500, description = "Internal error."),
    )
)]
pub(crate) async fn system_info(State(s): State<AppState>) -> ApiResult<SystemInfo> {
    let (tx, rx) = oneshot::channel();
    s.overlord.tx.send(Cmd::GetSystemInfo { reply: tx }).await?;
    ok(rx.await?)
}
