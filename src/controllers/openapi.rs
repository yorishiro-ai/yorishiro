//! Contract-only OpenAPI shapes and metadata for the community controllers.
//!
//! These types intentionally mirror the serialized HTTP wire format rather than domain models.

#![allow(dead_code)]

use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(ToSchema)]
pub(crate) struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(ToSchema)]
pub(crate) struct ApiErrorDetail {
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
pub(crate) struct ValidationDetail {
    pub field: String,
    pub problem: String,
    pub code: String,
    #[schema(nullable = true)]
    pub expected: Option<String>,
    #[schema(nullable = true)]
    pub actual: Option<String>,
}

#[derive(ToSchema)]
pub(crate) struct SignupRequest {
    #[schema(nullable = true)]
    pub invite_token: Option<String>,
    #[schema(nullable = true)]
    pub email: Option<String>,
    pub password: String,
    #[schema(nullable = true)]
    pub display_name: Option<String>,
}

#[derive(ToSchema)]
pub(crate) struct WorkspaceSummary {
    pub id: Uuid,
    pub name: String,
}

#[derive(ToSchema)]
pub(crate) struct SignupResponse {
    pub user_id: Uuid,
    pub email: String,
    pub tenant_id: Uuid,
    pub role: MembershipRole,
    pub workspaces: Vec<WorkspaceSummary>,
}

#[derive(ToSchema)]
pub(crate) struct LoginRequest {
    pub email: String,
    pub password: String,
    #[schema(nullable = true)]
    pub workspace_id: Option<Uuid>,
}

#[derive(ToSchema)]
pub(crate) struct LoginResponse {
    pub api_key: String,
    pub api_key_id: Uuid,
    pub workspace_id: Uuid,
    pub scope: ApiKeyScope,
    pub user_id: Uuid,
}

#[derive(ToSchema)]
pub(crate) struct SetupRequest {
    pub email: String,
    pub password: String,
    #[schema(nullable = true)]
    pub display_name: Option<String>,
}

#[derive(ToSchema)]
pub(crate) struct SetupResponse {
    pub user_id: Uuid,
    pub email: String,
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub api_key: String,
}

#[derive(ToSchema)]
pub(crate) struct SetupStatusResponse {
    pub setup_required: bool,
}

#[derive(ToSchema)]
pub(crate) struct EntityRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub schema_id: Uuid,
    pub schema_version: i32,
    pub entity_type: String,
    pub data: Value,
    #[schema(format = DateTime)]
    pub created_at: String,
    #[schema(format = DateTime)]
    pub updated_at: String,
    #[schema(nullable = true)]
    pub created_by: Option<Uuid>,
    #[schema(nullable = true)]
    pub updated_by: Option<Uuid>,
}

#[derive(ToSchema)]
pub(crate) struct SearchHit {
    pub entity: EntityRecord,
    #[schema(nullable = true)]
    pub distance: Option<f64>,
}

#[derive(ToSchema)]
pub(crate) struct CreateEntityRequest {
    pub schema_name: String,
    pub entity_type: String,
    pub data: Value,
}

#[derive(ToSchema)]
pub(crate) struct UpdateEntityRequest {
    pub data: Value,
}

#[derive(ToSchema)]
pub(crate) struct FillDefaultsRequest {
    pub schema_name: String,
}

#[derive(ToSchema)]
pub(crate) struct FillDefaultsResponse {
    pub schema_name: String,
    pub job_id: Uuid,
    pub entities_updated: i64,
    pub fields_filled: i64,
}

#[derive(ToSchema)]
pub(crate) struct ReindexResponse {
    pub job_id: String,
}

#[derive(ToSchema)]
pub(crate) struct ImportResult {
    pub schemas: usize,
    pub entities: usize,
    pub relations: usize,
}

#[derive(ToSchema)]
pub(crate) struct UndoReport {
    pub job_id: Uuid,
    pub restored: i64,
    pub missing: i64,
}

#[derive(ToSchema)]
pub(crate) struct RelationRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    pub properties: Value,
    pub status: RelationStatus,
    #[schema(format = DateTime)]
    pub created_at: String,
}

#[derive(ToSchema)]
pub(crate) struct CreateRelationRequest {
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    #[schema(nullable = true)]
    pub properties: Option<Value>,
}

#[derive(ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RelationStatus {
    Active,
    Deprecated,
    Archived,
}

#[derive(ToSchema)]
pub(crate) struct SetRelationStatusRequest {
    pub status: RelationStatus,
}

#[derive(ToSchema)]
pub(crate) struct SchemaRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub version: i32,
    pub definition: Value,
    pub status: SchemaStatus,
    #[schema(nullable = true)]
    pub origin_template_id: Option<Uuid>,
    pub origin_status: SchemaOriginStatus,
    #[schema(nullable = true)]
    pub origin_snapshot: Option<Value>,
    #[schema(nullable = true, format = DateTime)]
    pub origin_updated_at: Option<String>,
    #[schema(format = DateTime)]
    pub created_at: String,
}

#[derive(ToSchema)]
pub(crate) struct SchemaSummary {
    pub id: Uuid,
    pub name: String,
    pub version: i32,
    pub status: SchemaStatus,
    #[schema(format = DateTime)]
    pub created_at: String,
}

#[derive(ToSchema)]
pub(crate) struct CreateSchemaResponse {
    pub schema: SchemaRecord,
    pub diff: VersioningDiff,
}

#[derive(ToSchema)]
pub(crate) struct VersioningDiff {
    pub is_breaking: bool,
    pub reasons: Vec<String>,
}

#[derive(ToSchema)]
pub(crate) struct TemplateSummary {
    pub id: String,
    pub name: String,
    #[schema(nullable = true)]
    pub description: Option<String>,
}

#[derive(ToSchema)]
pub(crate) struct TemplateRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    #[schema(nullable = true)]
    pub description: Option<String>,
    pub definition: Value,
    pub tags: Vec<String>,
    #[schema(nullable = true)]
    pub locale: Option<String>,
    pub visibility: TemplateVisibility,
    #[schema(nullable = true)]
    pub author: Option<String>,
    #[schema(nullable = true)]
    pub fork_of: Option<Uuid>,
    #[schema(nullable = true)]
    pub created_by: Option<Uuid>,
    #[schema(format = DateTime)]
    pub created_at: String,
    #[schema(format = DateTime)]
    pub updated_at: String,
}

#[derive(ToSchema)]
pub(crate) struct CreateTemplateRequest {
    pub name: String,
    #[schema(nullable = true)]
    pub description: Option<String>,
    pub definition: Value,
    #[serde(default)]
    pub tags: Vec<String>,
    #[schema(nullable = true)]
    pub locale: Option<String>,
    #[schema(nullable = true)]
    pub author: Option<String>,
}

#[derive(ToSchema)]
pub(crate) struct UpdateTemplateRequest {
    #[schema(nullable = true)]
    pub name: Option<String>,
    #[schema(nullable = true)]
    pub description: Option<String>,
    #[schema(nullable = true)]
    pub definition: Option<Value>,
    #[schema(nullable = true)]
    pub tags: Option<Vec<String>>,
    #[schema(nullable = true)]
    pub locale: Option<String>,
}

#[derive(ToSchema)]
pub(crate) struct ForkTemplateRequest {
    pub name: String,
}

#[derive(ToSchema)]
pub(crate) struct MembershipRecord {
    pub user_id: Uuid,
    pub email: String,
    #[schema(nullable = true)]
    pub display_name: Option<String>,
    pub role: MembershipRole,
}

#[derive(ToSchema)]
pub(crate) struct AuditLogRecord {
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
pub(crate) struct AddMemberRequest {
    pub email: String,
    pub role: MembershipRole,
}

#[derive(ToSchema)]
pub(crate) struct CreateWorkspaceRequest {
    pub name: String,
    #[schema(nullable = true)]
    pub max_entities: Option<i32>,
    #[schema(nullable = true)]
    pub schema_id: Option<Uuid>,
}

#[derive(ToSchema)]
pub(crate) struct WorkspaceRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    #[schema(nullable = true)]
    pub max_entities: Option<i32>,
    #[schema(nullable = true)]
    pub embedding_model: Option<String>,
    #[schema(nullable = true)]
    pub embedding_dimensions: Option<i32>,
    #[schema(nullable = true)]
    pub schema_id: Option<Uuid>,
    pub status: WorkspaceStatus,
    #[schema(format = DateTime)]
    pub created_at: String,
}

#[derive(ToSchema)]
pub(crate) struct WorkspaceDetail {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    #[schema(nullable = true)]
    pub max_entities: Option<i32>,
    #[schema(nullable = true)]
    pub schema_id: Option<Uuid>,
    #[schema(format = DateTime)]
    pub created_at: String,
    pub entity_count: i64,
    pub relation_count: i64,
    pub schema_count: i64,
}

#[derive(ToSchema)]
pub(crate) struct MaintenanceResponse {
    pub mode: MaintenanceMode,
    pub retry_after: u32,
    #[schema(nullable = true)]
    pub reason: Option<String>,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MaintenanceMode {
    Off,
    ReadOnly,
    FullLock,
}

#[derive(ToSchema)]
pub(crate) struct SetMaintenanceRequest {
    pub mode: MaintenanceMode,
    #[schema(nullable = true)]
    pub retry_after: Option<u32>,
    #[schema(nullable = true)]
    pub reason: Option<String>,
}

#[derive(ToSchema)]
pub(crate) struct WhoAmIResponse {
    pub workspace_id: Uuid,
    pub tenant_id: Uuid,
    pub scope: ApiKeyScope,
    #[schema(nullable = true)]
    pub user_id: Option<Uuid>,
    pub audit: bool,
}

#[derive(ToSchema)]
pub(crate) struct OAuthStatus {
    pub enabled: bool,
}

#[derive(ToSchema)]
pub(crate) struct ForkResponse {
    pub template_id: Uuid,
}

#[derive(ToSchema)]
pub(crate) struct SetVisibilityRequest {
    pub visibility: TemplateVisibility,
}

#[derive(ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MembershipRole {
    Owner,
    Admin,
    Member,
    Viewer,
}

#[derive(ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ApiKeyScope {
    Read,
    Write,
    Schema,
    Migration,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SchemaStatus {
    Active,
    Archived,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SchemaOriginStatus {
    Linked,
    Detached,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkspaceStatus {
    SchemaPending,
    Active,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuditAction {
    UndoMigrationJob,
    SetMaintenance,
    ReindexEmbeddings,
    FillDefaults,
}

#[derive(ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TemplateVisibility {
    Tenant,
    Community,
}

#[derive(ToSchema)]
pub(crate) struct JsonSchemaDefinition {
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
pub(crate) enum CreateSchemaRequest {
    Definition(JsonSchemaDefinition),
    Template { template_id: String },
}

#[derive(ToSchema)]
pub(crate) struct EmbeddingKeyRequest {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub dimensions: i32,
    #[serde(default)]
    pub send_dimensions_param: bool,
}

#[derive(ToSchema)]
pub(crate) struct EmbeddingKeyResponse {
    pub base_url: String,
    pub model: String,
    pub dimensions: i32,
    pub configured: bool,
}

#[derive(ToSchema)]
pub(crate) struct LlmKeyRequest {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

#[derive(ToSchema)]
pub(crate) struct LlmKeyResponse {
    pub base_url: String,
    pub model: String,
    pub configured: bool,
}

#[derive(ToSchema)]
pub(crate) struct WorkerClassRequest {
    pub worker_class: WorkerClass,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkerClass {
    TenantPrivate,
    Official,
    Shared,
}

#[derive(ToSchema)]
pub(crate) struct WorkerClassResponse {
    pub worker_class: WorkerClass,
}

#[derive(ToSchema)]
pub(crate) struct ColumnPreference {
    pub entity_type: String,
    pub columns: Vec<String>,
}

#[derive(ToSchema)]
pub(crate) struct SetColumnsRequest {
    pub columns: Vec<String>,
}

#[derive(ToSchema)]
pub(crate) struct InferFillResponse {
    pub job_id: String,
    pub status: InferFillStatus,
}

#[derive(ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum InferFillStatus {
    Queued,
}

#[derive(ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum InferJobStatusValue {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(ToSchema)]
pub(crate) struct InferJobStatusResponse {
    pub job_id: String,
    pub status: InferJobStatusValue,
    #[schema(nullable = true)]
    pub applied: Option<i64>,
    #[schema(nullable = true)]
    pub skipped: Option<i64>,
    #[schema(nullable = true)]
    pub error: Option<String>,
}

#[derive(ToSchema)]
pub(crate) struct TenantUsage {
    pub tenant_id: Uuid,
    pub workspace_count: i64,
    pub member_count: i64,
    pub entity_count: i64,
}

#[derive(ToSchema)]
pub(crate) struct TenantOverview {
    pub tenant_id: Uuid,
    #[schema(nullable = true)]
    pub plan: Option<String>,
    #[schema(nullable = true)]
    pub max_workspaces: Option<i32>,
    pub usage: TenantUsage,
    pub members: Vec<MembershipRecord>,
}

#[derive(ToSchema)]
pub(crate) struct MarketplaceListing {
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
pub(crate) struct TemplateVersionRecord {
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
pub(crate) enum TemplateVersionStatus {
    Draft,
    Pre,
    Stable,
}

#[derive(ToSchema)]
pub(crate) struct PublishVersionRequest {
    #[schema(value_type = Object)]
    pub definition: Value,
    #[schema(nullable = true)]
    pub changelog: Option<String>,
    #[schema(required = false, pattern = "^(draft|pre|stable)$", default = "draft")]
    #[serde(default)]
    pub status: String,
}

#[derive(ToSchema)]
pub(crate) struct TemplateReviewRecord {
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
pub(crate) struct SubmitReviewRequest {
    pub rating: i16,
    #[schema(nullable = true)]
    pub comment: Option<String>,
}

#[derive(ToSchema)]
pub(crate) struct ForkRecord {
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
pub(crate) struct CreateForkRequest {
    pub source_workspace_id: Uuid,
    pub source_schema_id: Uuid,
}

#[derive(ToSchema)]
pub(crate) struct UpdateForkRequest {
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
pub(crate) struct UpstreamChange {
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
pub(crate) struct MergePlan {
    pub fields: Vec<FieldMerge>,
    pub summary: MergeDiffSummary,
}

#[derive(ToSchema)]
pub(crate) struct FieldMerge {
    pub entity_type: String,
    pub field: String,
    pub verdict: MergeVerdict,
    pub detail: String,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MergeVerdict {
    AutoAdd,
    AutoUpdate,
    KeepLocal,
    Conflict,
}

#[derive(ToSchema)]
pub(crate) struct MergeDiffSummary {
    pub total_fields: usize,
    pub auto_add: usize,
    pub auto_update: usize,
    pub keep_local: usize,
    pub conflict: usize,
    pub has_conflicts: bool,
}

#[derive(ToSchema)]
pub(crate) struct MergeResponse {
    pub schema: SchemaRecord,
    pub diff: MergeDiff,
    pub summary: MergeDiffSummary,
}

#[derive(ToSchema)]
pub(crate) struct MergeDiff {
    pub is_breaking: bool,
    pub reasons: Vec<String>,
}

pub(crate) type JsonSchema = Value;
