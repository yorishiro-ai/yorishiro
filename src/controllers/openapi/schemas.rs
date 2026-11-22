//! Contract-only `OpenAPI` shapes and metadata for the community controllers.
//!
//! These types intentionally mirror the serialized HTTP wire format rather than domain models.

use schemars::JsonSchema as Schema;

use crate::error::ValidationDetail;

#[derive(Schema)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(Schema)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
    pub details: Option<Vec<ValidationDetail>>,
    pub hint: Option<String>,
    pub retry_after_seconds: Option<u32>,
}
