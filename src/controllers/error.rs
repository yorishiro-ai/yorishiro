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

impl aide::OperationOutput for ApiError {
    type Inner = ();

    /// The statuses `YorishiroError` maps to besides authorization.
    fn inferred_responses(
        _: &mut aide::generate::GenContext,
        _: &mut aide::openapi::Operation,
    ) -> Vec<(Option<aide::openapi::StatusCode>, aide::openapi::Response)> {
        [404, 409, 422]
            .map(|code| {
                (
                    Some(aide::openapi::StatusCode::Code(code)),
                    aide::openapi::Response::default(),
                )
            })
            .into()
    }
}
