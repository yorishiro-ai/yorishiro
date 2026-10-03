//! Contract-only OpenAPI shapes and metadata for the community controllers.
//!
//! These types intentionally mirror the serialized HTTP wire format rather than domain models.

use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::ValidationDetail;

use crate::models::api_key_audit_log::AuditAction;

#[derive(ToSchema)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(ToSchema)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
    #[schema(nullable = true)]
    pub details: Option<Vec<ValidationDetail>>,
    #[schema(nullable = true)]
    pub hint: Option<String>,
    #[schema(nullable = true)]
    pub retry_after_seconds: Option<u32>,
}

#[derive(ToSchema)]
pub struct AuditLogRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub tenant_id: Uuid,
    #[schema(nullable = true)]
    pub api_key_id: Option<Uuid>,
    #[schema(nullable = true)]
    pub user_id: Option<Uuid>,
    pub action: AuditAction,
    pub detail: Value,
    #[schema(format = DateTime)]
    pub created_at: String,
}

#[derive(ToSchema)]
pub struct JsonSchemaDefinition {
    pub name: String,
    #[schema(nullable = true)]
    pub description: Option<String>,
    #[schema(value_type = Object)]
    pub entity_types: Value,
    #[schema(required = false, value_type = Object)]
    pub relation_types: Value,
}

#[derive(ToSchema)]
#[serde(untagged)]
pub enum CreateSchemaRequest {
    Definition(JsonSchemaDefinition),
    Template { template_id: String },
}

pub type JsonSchema = Value;
