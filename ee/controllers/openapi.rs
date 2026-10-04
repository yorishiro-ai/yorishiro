//! Contract-only OpenAPI shapes for the enterprise controllers.
//!
//! These types intentionally mirror the serialized HTTP wire format rather than domain models.

use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::models::schema_schemas::SchemaRecord;
use crate::models::template_templates::TemplateVisibility;
use crate::models::tenant_memberships::MembershipRecord;

#[derive(ToSchema)]
pub struct OAuthStatus {
    pub enabled: bool,
}

#[derive(ToSchema)]
pub struct ForkResponse {
    pub template_id: Uuid,
}

#[derive(ToSchema)]
pub struct SetVisibilityRequest {
    pub visibility: TemplateVisibility,
}

#[derive(ToSchema)]
pub struct EmbeddingKeyRequest {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub dimensions: i32,
    #[serde(default)]
    pub send_dimensions_param: bool,
}

#[derive(ToSchema)]
pub struct EmbeddingKeyResponse {
    pub base_url: String,
    pub model: String,
    pub dimensions: i32,
    pub configured: bool,
}

#[derive(ToSchema)]
pub struct LlmKeyRequest {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

#[derive(ToSchema)]
pub struct LlmKeyResponse {
    pub base_url: String,
    pub model: String,
    pub configured: bool,
}

#[derive(ToSchema)]
pub struct WorkerClassRequest {
    pub worker_class: WorkerClass,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkerClass {
    TenantPrivate,
    Official,
    Shared,
}

#[derive(ToSchema)]
pub struct WorkerClassResponse {
    pub worker_class: WorkerClass,
}

#[derive(ToSchema)]
pub struct ColumnPreference {
    pub entity_type: String,
    pub columns: Vec<String>,
}

#[derive(ToSchema)]
pub struct SetColumnsRequest {
    pub columns: Vec<String>,
}

#[derive(ToSchema)]
pub struct InferFillResponse {
    pub job_id: String,
    pub status: InferFillStatus,
}

#[derive(ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum InferFillStatus {
    Queued,
}

#[derive(ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum InferJobStatusValue {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(ToSchema)]
pub struct InferJobStatusResponse {
    pub job_id: String,
    pub status: InferJobStatusValue,
    #[schema(nullable = true)]
    pub applied: Option<i64>,
    #[schema(nullable = true)]
    pub proposed: Option<i64>,
    #[schema(nullable = true)]
    pub skipped: Option<i64>,
    #[schema(nullable = true)]
    pub error: Option<String>,
}

#[derive(ToSchema)]
pub struct InferenceProposalResponse {
    pub id: Uuid,
    pub job_id: Uuid,
    pub workspace_id: Uuid,
    pub entity_id: Uuid,
    pub schema_id: Uuid,
    pub schema_version: i32,
    pub source_field: String,
    pub proposed: Value,
    pub status: String,
}

#[derive(ToSchema)]
pub struct ProposalActionResponse {
    pub job_id: Uuid,
    pub changed: i64,
}

#[derive(ToSchema)]
pub struct ProposalConfirmResponse {
    pub job_id: Uuid,
    pub confirmed: i64,
    pub stale: i64,
    pub invalid: i64,
}

#[derive(ToSchema)]
pub struct TenantUsage {
    pub tenant_id: Uuid,
    pub workspace_count: i64,
    pub member_count: i64,
    pub entity_count: i64,
}

#[derive(ToSchema)]
pub struct TenantOverview {
    pub tenant_id: Uuid,
    #[schema(nullable = true)]
    pub plan: Option<String>,
    #[schema(nullable = true)]
    pub max_workspaces: Option<i32>,
    pub usage: TenantUsage,
    pub members: Vec<MembershipRecord>,
}

#[derive(ToSchema)]
pub struct MarketplaceListing {
    pub template_id: Uuid,
    pub name: String,
    #[schema(nullable = true)]
    pub description: Option<String>,
    pub tags: Vec<String>,
    #[schema(nullable = true)]
    pub author: Option<String>,
    pub tenant_id: Uuid,
    #[schema(nullable = true)]
    pub latest_stable_version: Option<i32>,
    pub review_count: i64,
    #[schema(nullable = true)]
    pub average_rating: Option<f64>,
}

#[derive(ToSchema)]
pub struct TemplateVersionRecord {
    pub id: Uuid,
    pub template_id: Uuid,
    pub version: i32,
    #[schema(value_type = Object)]
    pub definition: Value,
    #[schema(nullable = true)]
    pub changelog: Option<String>,
    pub status: TemplateVersionStatus,
    #[schema(format = DateTime)]
    pub created_at: String,
}

#[derive(ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TemplateVersionStatus {
    Draft,
    Pre,
    Stable,
}

#[derive(ToSchema)]
pub struct PublishVersionRequest {
    #[schema(value_type = Object)]
    pub definition: Value,
    #[schema(nullable = true)]
    pub changelog: Option<String>,
    #[schema(required = false, pattern = "^(draft|pre|stable)$", default = "draft")]
    #[serde(default)]
    pub status: String,
}

#[derive(ToSchema)]
pub struct TemplateReviewRecord {
    pub id: Uuid,
    pub template_id: Uuid,
    pub tenant_id: Uuid,
    pub rating: i16,
    #[schema(nullable = true)]
    pub comment: Option<String>,
    #[schema(format = DateTime)]
    pub created_at: String,
    #[schema(format = DateTime)]
    pub updated_at: String,
}

#[derive(ToSchema)]
pub struct SubmitReviewRequest {
    pub rating: i16,
    #[schema(nullable = true)]
    pub comment: Option<String>,
}

#[derive(ToSchema)]
pub struct ForkRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub source_workspace_id: Uuid,
    pub source_schema_id: Uuid,
    pub source_schema_version: i32,
    pub source_schema_name: String,
    pub fork_schema_id: Uuid,
    pub fork_schema_version: i32,
    pub customized: bool,
    #[schema(nullable = true)]
    pub upstream_version: Option<i32>,
    #[schema(value_type = Object)]
    pub definition: Value,
    #[schema(format = DateTime)]
    pub created_at: String,
    #[schema(format = DateTime)]
    pub updated_at: String,
}

#[derive(ToSchema)]
pub struct CreateForkRequest {
    pub source_workspace_id: Uuid,
    pub source_schema_id: Uuid,
}

#[derive(ToSchema)]
pub struct UpdateForkRequest {
    #[schema(nullable = true)]
    pub definition: Option<Value>,
    #[schema(nullable = true)]
    pub action: Option<String>,
    #[serde(default)]
    pub force: bool,
    #[schema(nullable = true)]
    pub expected_fork_schema_id: Option<Uuid>,
    #[schema(nullable = true)]
    pub expected_source_schema_id: Option<Uuid>,
}

#[derive(ToSchema)]
pub struct UpstreamChange {
    pub schema_id: Uuid,
    pub schema_name: String,
    pub version: i32,
    pub template_id: Uuid,
    pub template_name: String,
    #[schema(format = DateTime)]
    pub changed_at: String,
    pub pending_notification: bool,
    pub summary: MergeDiffSummary,
}

#[derive(ToSchema)]
pub struct MergePlan {
    pub fields: Vec<FieldMerge>,
    pub summary: MergeDiffSummary,
}

#[derive(ToSchema)]
pub struct FieldMerge {
    pub entity_type: String,
    pub field: String,
    pub verdict: MergeVerdict,
    pub detail: String,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeVerdict {
    AutoAdd,
    AutoUpdate,
    KeepLocal,
    Conflict,
}

#[derive(ToSchema)]
pub struct MergeDiffSummary {
    pub total_fields: usize,
    pub auto_add: usize,
    pub auto_update: usize,
    pub keep_local: usize,
    pub conflict: usize,
    pub has_conflicts: bool,
}

#[derive(ToSchema)]
pub struct MergeResponse {
    pub schema: SchemaRecord,
    pub diff: MergeDiff,
    pub summary: MergeDiffSummary,
}

#[derive(ToSchema)]
pub struct MergeDiff {
    pub is_breaking: bool,
    pub reasons: Vec<String>,
}
