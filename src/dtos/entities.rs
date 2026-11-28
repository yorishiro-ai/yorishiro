use crate::models::entity_entities;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CreateEntityRequest {
    /// Name of the schema this entity conforms to.
    /// The workspace's current active version is used.
    pub schema_name: String,
    #[expect(clippy::doc_markdown, reason = "MCP exposes this description verbatim")]
    /// entity_type name declared in the schema.
    pub entity_type: String,
    /// Entity body (JSON object) conforming to the schema's `fields` definition.
    pub data: Value,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct FillDefaultsRequest {
    /// Name of the schema whose active version supplies the missing fields.
    pub(crate) schema_name: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct FillDefaultsResponse {
    pub(crate) schema_name: String,
    pub(crate) migration_job_id: Uuid,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct UndoResponse {
    pub(crate) migration_job_id: Uuid,
    pub(crate) snapshot_job_id: Uuid,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateEntityRequest {
    pub(crate) data: Value,
    /// The entity's `updated_at` as the client read it; a mismatch is refused with a conflict so a stale writer cannot clobber a newer write.
    #[serde(default)]
    pub(crate) expected_updated_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ListEntitiesParams {
    pub entity_type: Option<String>,
    /// JSON-encoded containment filter, e.g. `{"status":"active"}`.
    pub filter: Option<String>,
    /// Restricts results to entities created against this schema version.
    pub schema_version: Option<i32>,
    #[serde(flatten)]
    pub page: crate::dtos::common::PageParams,
}

impl From<CreateEntityRequest> for entity_entities::CreateEntityInput {
    fn from(request: CreateEntityRequest) -> Self {
        Self {
            schema_name: request.schema_name,
            entity_type: request.entity_type,
            data: request.data,
        }
    }
}

impl TryFrom<ListEntitiesParams> for entity_entities::ListEntitiesQuery {
    type Error = crate::YorishiroError;

    fn try_from(params: ListEntitiesParams) -> Result<Self, Self::Error> {
        Ok(Self {
            entity_type: params.entity_type,
            filter: crate::dtos::common::parse_filter_param(params.filter)?,
            schema_version: params.schema_version,
            page: params.page.try_into()?,
        })
    }
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct ReindexResponse {
    /// The job ID assigned by the queue provider.
    pub(crate) migration_job_id: String,
}
