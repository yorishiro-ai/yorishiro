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

/// A JSON body with a fixed success status, in the response and in the `OpenAPI` document.
macro_rules! json_with_status {
    ($(#[$doc:meta])* $name:ident, $status:ident, $code:literal) => {
        $(#[$doc])*
        pub struct $name<T>(pub T);

        impl<T: serde::Serialize> IntoResponse for $name<T> {
            fn into_response(self) -> Response {
                (axum::http::StatusCode::$status, Json(self.0)).into_response()
            }
        }

        impl<T: schemars::JsonSchema> aide::OperationOutput for $name<T> {
            type Inner = T;

            fn inferred_responses(
                ctx: &mut aide::generate::GenContext,
                operation: &mut aide::openapi::Operation,
            ) -> Vec<(Option<aide::openapi::StatusCode>, aide::openapi::Response)> {
                <Json<T> as aide::OperationOutput>::operation_response(ctx, operation)
                    .into_iter()
                    .map(|response| (Some(aide::openapi::StatusCode::Code($code)), response))
                    .collect()
            }
        }
    };
}

json_with_status!(
    /// `201 Created` with a JSON body.
    Created,
    CREATED,
    201
);
json_with_status!(
    /// `202 Accepted` with a JSON body.
    Accepted,
    ACCEPTED,
    202
);
