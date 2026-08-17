use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use yorishiro_core::YorishiroError;

/// The JSON envelope every failing route in this crate returns, for OpenAPI only: nothing
/// constructs it. The response body is built by `YorishiroError::into_http_parts()` as an
/// untyped `serde_json::Value`, so there is no existing Rust type describing its shape, and
/// `yorishiro-server`'s equivalent (`ApiErrorBody`) lives behind a `pub(crate)` module this
/// crate cannot reach. Kept in sync by hand with core's `into_http_parts`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct HostedApiErrorBody {
    pub error: HostedApiErrorDetail,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct HostedApiErrorDetail {
    pub message: String,
    /// Present on validation failures (`422`) only. Omitted rather than serialised as `null`
    /// when absent, matching what `into_http_parts` actually emits: a `403` body carries
    /// `message` and `hint` and no `details` key at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// A suggested fix, on the variants that can offer one (`422`, `403`). Omitted when absent,
    /// for the same reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// A thin wrapper that converts `YorishiroError` into an HTTP response. The actual
/// status/body mapping is defined once in `YorishiroError::into_http_parts()` (in
/// yorishiro-core) and shared with `yorishiro-server`'s `ApiError`. This crate only
/// needs the newtype so axum's `IntoResponse` impl can be provided here without an
/// orphan-rule conflict.
pub struct HostedApiError(pub YorishiroError);

impl From<YorishiroError> for HostedApiError {
    fn from(err: YorishiroError) -> Self {
        Self(err)
    }
}

impl IntoResponse for HostedApiError {
    fn into_response(self) -> Response {
        let (status_u16, body) = self.0.into_http_parts();
        let status = StatusCode::from_u16(status_u16).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
#[path = "../tests/error.rs"]
mod tests;
