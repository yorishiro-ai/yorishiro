//! Contract-only `OpenAPI` shapes and metadata for the community controllers.
//!
//! These types intentionally mirror the serialized HTTP wire format rather than domain models.

use schemars::JsonSchema as Schema;
use serde_json::Value;

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

#[derive(Schema)]
pub struct JsonSchemaDefinition {
    pub name: String,
    pub description: Option<String>,
    #[schemars(with = "Value")]
    pub entity_types: Value,
    #[schemars(with = "Value")]
    pub relation_types: Value,
}

#[derive(Schema)]
#[serde(untagged)]
pub enum CreateSchemaRequest {
    Definition(JsonSchemaDefinition),
    Template { template_id: String },
}

pub type JsonSchema = Value;
