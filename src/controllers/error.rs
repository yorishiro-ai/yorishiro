use axum::Json;
use axum::response::{IntoResponse, Response};

use crate::error::YorishiroError;

/// Newtype wrapper over `YorishiroError` for axum.
/// The name is fixed; do not rename.
pub struct ApiError(pub YorishiroError);

impl From<YorishiroError> for ApiError {
    fn from(err: YorishiroError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = self.0.into_http_parts();
        (status, Json(body)).into_response()
    }
}
