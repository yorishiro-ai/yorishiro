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
        crate::controllers::middleware::access_log::failed(&self.0);
        let (status, body) = self.0.into_http_parts();
        (status, Json(body)).into_response()
    }
}

impl aide::OperationOutput for ApiError {
    type Inner = ();

    fn inferred_responses(
        ctx: &mut aide::generate::GenContext,
        operation: &mut aide::openapi::Operation,
    ) -> Vec<(Option<aide::openapi::StatusCode>, aide::openapi::Response)> {
        crate::controllers::openapi::error_responses(
            ctx,
            operation,
            &[401, 403, 404, 409, 422, 423, 500, 501, 502, 503],
        )
    }
}

/// `201 Created` with a JSON body.
pub struct Created<T>(pub T);

impl<T: serde::Serialize> IntoResponse for Created<T> {
    fn into_response(self) -> Response {
        (axum::http::StatusCode::CREATED, Json(self.0)).into_response()
    }
}

impl<T: schemars::JsonSchema> aide::OperationOutput for Created<T> {
    type Inner = T;

    fn inferred_responses(
        ctx: &mut aide::generate::GenContext,
        operation: &mut aide::openapi::Operation,
    ) -> Vec<(Option<aide::openapi::StatusCode>, aide::openapi::Response)> {
        <Json<T> as aide::OperationOutput>::operation_response(ctx, operation)
            .into_iter()
            .map(|response| (Some(aide::openapi::StatusCode::Code(201)), response))
            .collect()
    }
}

/// `202 Accepted` with a JSON body.
pub struct Accepted<T>(pub T);

impl<T: serde::Serialize> IntoResponse for Accepted<T> {
    fn into_response(self) -> Response {
        (axum::http::StatusCode::ACCEPTED, Json(self.0)).into_response()
    }
}

impl<T: schemars::JsonSchema> aide::OperationOutput for Accepted<T> {
    type Inner = T;

    fn inferred_responses(
        ctx: &mut aide::generate::GenContext,
        operation: &mut aide::openapi::Operation,
    ) -> Vec<(Option<aide::openapi::StatusCode>, aide::openapi::Response)> {
        <Json<T> as aide::OperationOutput>::operation_response(ctx, operation)
            .into_iter()
            .map(|response| (Some(aide::openapi::StatusCode::Code(202)), response))
            .collect()
    }
}
