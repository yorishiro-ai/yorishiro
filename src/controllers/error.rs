use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::error::YorishiroError;

/// Newtype wrapper over `YorishiroError` for axum. The name is fixed; do not rename.
pub struct ApiError(pub YorishiroError);

impl From<YorishiroError> for ApiError {
    fn from(err: YorishiroError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = self.0.into_http_parts();
        let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(body)).into_response()
    }
}
