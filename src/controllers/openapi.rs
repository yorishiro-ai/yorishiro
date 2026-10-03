//! Contract-only OpenAPI shapes and metadata for the community controllers.
//!
//! These types intentionally mirror the serialized HTTP wire format rather than domain models.

use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

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
pub struct ValidationDetail {
    pub field: String,
    pub problem: String,
    pub code: String,
    #[schema(nullable = true)]
    pub expected: Option<String>,
    #[schema(nullable = true)]
    pub actual: Option<String>,
}

#[derive(ToSchema)]
pub struct SignupRequest {
    #[schema(nullable = true)]
    pub invite_token: Option<String>,
    #[schema(nullable = true)]
    pub email: Option<String>,
    pub password: String,
    #[schema(nullable = true)]
    pub display_name: Option<String>,
}

#[derive(ToSchema)]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub name: String,
}

#[derive(ToSchema)]
pub struct SignupResponse {
    pub user_id: Uuid,
    pub email: String,
    pub tenant_id: Uuid,
    pub role: MembershipRole,
    pub workspaces: Vec<WorkspaceSummary>,
}

#[derive(ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    #[schema(nullable = true)]
    pub workspace_id: Option<Uuid>,
}

#[derive(ToSchema)]
pub struct LoginResponse {
    pub api_key: String,
    pub api_key_id: Uuid,
    pub workspace_id: Uuid,
    pub scope: ApiKeyScope,
    pub user_id: Uuid,
}

#[derive(ToSchema)]
pub struct SetupRequest {
    pub email: String,
    pub password: String,
    #[schema(nullable = true)]
    pub display_name: Option<String>,
}

#[derive(ToSchema)]
pub struct SetupResponse {
    pub user_id: Uuid,
    pub email: String,
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub api_key: String,
}

#[derive(ToSchema)]
pub struct SetupStatusResponse {
    pub setup_required: bool,
}

#[derive(ToSchema)]
pub struct EntityRecord {
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
pub struct SearchHit {
    pub entity: EntityRecord,
    #[schema(nullable = true)]
    pub distance: Option<f64>,
}

#[derive(ToSchema)]
pub struct CreateEntityRequest {
    pub schema_name: String,
    pub entity_type: String,
    pub data: Value,
}

#[derive(ToSchema)]
pub struct UpdateEntityRequest {
    pub data: Value,
}

#[derive(ToSchema)]
pub struct FillDefaultsRequest {
    pub schema_name: String,
}

#[derive(ToSchema)]
pub struct FillDefaultsResponse {
    pub schema_name: String,
    pub job_id: Uuid,
    pub entities_updated: i64,
    pub fields_filled: i64,
}

#[derive(ToSchema)]
pub struct ReindexResponse {
    pub job_id: String,
}

#[derive(ToSchema)]
pub struct ImportResult {
    pub schemas: usize,
    pub entities: usize,
    pub relations: usize,
}

#[derive(ToSchema)]
pub struct UndoReport {
    pub job_id: Uuid,
    pub restored: i64,
    pub missing: i64,
}

#[derive(ToSchema)]
pub struct RelationRecord {
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
pub struct CreateRelationRequest {
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    #[schema(nullable = true)]
    pub properties: Option<Value>,
}

#[derive(ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RelationStatus {
    Active,
    Deprecated,
    Archived,
}

#[derive(ToSchema)]
pub struct SetRelationStatusRequest {
    pub status: RelationStatus,
}

#[derive(ToSchema)]
pub struct SchemaRecord {
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
pub struct SchemaSummary {
    pub id: Uuid,
    pub name: String,
    pub version: i32,
    pub status: SchemaStatus,
    #[schema(format = DateTime)]
    pub created_at: String,
}

#[derive(ToSchema)]
pub struct CreateSchemaResponse {
    pub schema: SchemaRecord,
    pub diff: VersioningDiff,
}

#[derive(ToSchema)]
pub struct VersioningDiff {
    pub is_breaking: bool,
    pub reasons: Vec<String>,
}

#[derive(ToSchema)]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    #[schema(nullable = true)]
    pub description: Option<String>,
}

#[derive(ToSchema)]
pub struct TemplateRecord {
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
pub struct CreateTemplateRequest {
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
pub struct UpdateTemplateRequest {
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
pub struct ForkTemplateRequest {
    pub name: String,
}

#[derive(ToSchema)]
pub struct MembershipRecord {
    pub user_id: Uuid,
    pub email: String,
    #[schema(nullable = true)]
    pub display_name: Option<String>,
    pub role: MembershipRole,
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
pub struct AddMemberRequest {
    pub email: String,
    pub role: MembershipRole,
}

#[derive(ToSchema)]
pub struct CreateApiKeyRequest {
    pub name: String,
}

#[derive(ToSchema)]
pub struct CreateApiKeyResponse {
    pub id: Uuid,
    pub prefix: String,
    pub full_key: String,
    pub name: String,
    #[schema(format = DateTime)]
    pub created_at: String,
}

#[derive(ToSchema)]
pub struct ApiKeyRecord {
    pub id: Uuid,
    pub prefix: String,
    pub name: String,
    #[schema(format = DateTime)]
    pub created_at: String,
}

#[derive(ToSchema)]
pub struct ListApiKeysResponse {
    pub keys: Vec<ApiKeyRecord>,
}

#[derive(ToSchema)]
pub struct CreateWorkspaceRequest {
    pub name: String,
    #[schema(nullable = true)]
    pub max_entities: Option<i32>,
    #[schema(nullable = true)]
    pub schema_id: Option<Uuid>,
}

#[derive(ToSchema)]
pub struct WorkspaceRecord {
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
pub struct WorkspaceDetail {
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
pub struct MaintenanceResponse {
    pub mode: MaintenanceMode,
    pub retry_after: u32,
    #[schema(nullable = true)]
    pub reason: Option<String>,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceMode {
    Off,
    ReadOnly,
    FullLock,
}

#[derive(ToSchema)]
pub struct SetMaintenanceRequest {
    pub mode: MaintenanceMode,
    #[schema(nullable = true)]
    pub retry_after: Option<u32>,
    #[schema(nullable = true)]
    pub reason: Option<String>,
}

#[derive(ToSchema)]
pub struct WhoAmIResponse {
    pub workspace_id: Uuid,
    pub tenant_id: Uuid,
    pub scope: ApiKeyScope,
    #[schema(nullable = true)]
    pub user_id: Option<Uuid>,
    pub audit: bool,
}

#[derive(ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MembershipRole {
    Owner,
    Admin,
    Member,
    Viewer,
}

#[derive(ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyScope {
    Read,
    Write,
    Schema,
    Migration,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SchemaStatus {
    Active,
    Archived,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SchemaOriginStatus {
    Linked,
    Detached,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    SchemaPending,
    Active,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    UndoMigrationJob,
    SetMaintenance,
    ReindexEmbeddings,
    FillDefaults,
}

#[derive(ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TemplateVisibility {
    Tenant,
    Community,
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
