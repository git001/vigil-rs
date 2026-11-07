// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Handler results: the response envelope and `ApiError`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use vigil_types::api::Response;

pub(crate) struct ApiError(StatusCode, anyhow::Error);

impl ApiError {
    pub(crate) fn forbidden() -> Self {
        ApiError(StatusCode::FORBIDDEN, anyhow::anyhow!("forbidden"))
    }

    /// Converts a `Caller::require` error into an `ApiError`.
    /// Signature matches `Result::map_err`'s closure expectation.
    pub(crate) fn forbidden_from(_: (axum::http::StatusCode, &'static str)) -> Self {
        Self::forbidden()
    }

    pub(crate) fn not_found(msg: impl Into<String>) -> Self {
        ApiError(StatusCode::NOT_FOUND, anyhow::anyhow!("{}", msg.into()))
    }

    /// `400 Bad Request`, e.g. an unknown signal name.
    pub(crate) fn bad_request(e: anyhow::Error) -> Self {
        ApiError(StatusCode::BAD_REQUEST, e)
    }

    /// Any status, for handlers that map their own errors.
    pub(crate) fn with_status(code: StatusCode, e: anyhow::Error) -> Self {
        ApiError(code, e)
    }

    /// `409 Conflict`, e.g. a change to an identity from a layer file.
    pub(crate) fn conflict(e: anyhow::Error) -> Self {
        ApiError(StatusCode::CONFLICT, e)
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let code = self.0.as_u16();
        let status_text = self.0.canonical_reason().unwrap_or("Error");
        let body = Json(serde_json::json!({
            "type": "error",
            "status-code": code,
            "status": status_text,
            "result": null,
            // `{:#}`: the whole context chain, not just the outermost
            // message (e.g. `service "web": invalid pass-env-regex …`).
            "message": format!("{:#}", self.1)
        }));
        (self.0, body).into_response()
    }
}

pub(crate) type ApiResult<T> = Result<Json<Response<T>>, ApiError>;

pub(crate) fn ok<T>(val: T) -> ApiResult<T> {
    Ok(Json(Response {
        r#type: "sync".into(),
        status_code: 200,
        status: "OK".into(),
        result: Some(val),
        message: None,
    }))
}
