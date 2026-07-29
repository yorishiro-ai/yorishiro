use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;
use yorishiro_core::YorishiroError;
use yorishiro_core::error::ValidationDetail;

/// A thin wrapper that converts `YorishiroError` into an HTTP response. The core split is
/// between client-caused errors (4xx, safe to return details for) and internal errors
/// (5xx, whose details go only to logs and never to the client).
pub struct ApiError(pub YorishiroError);

impl From<YorishiroError> for ApiError {
    fn from(err: YorishiroError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status_u16, body) = self.0.into_http_parts();
        let status = StatusCode::from_u16(status_u16).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(body)).into_response()
    }
}

/// A DTO that exists only to describe the error response shape in the OpenAPI document.
/// Actual response bodies are built individually by `ApiError::into_response`, so this
/// type's values are never used — it exists purely for schema generation.
#[derive(Serialize, ToSchema)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(Serialize, ToSchema)]
pub struct ApiErrorDetail {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<ValidationDetail>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}
