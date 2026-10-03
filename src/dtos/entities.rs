use crate::models::entity_entities::{self};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct CreateEntityRequest {
    pub schema_name: String,
    pub entity_type: String,
    pub data: Value,
}

#[derive(Deserialize)]
pub struct FillDefaultsRequest {
    pub schema_name: String,
}

#[derive(Serialize)]
pub struct FillDefaultsResponse {
    pub schema_name: String,
    pub job_id: Uuid,
    pub entities_updated: i64,
    pub fields_filled: i64,
}

#[derive(Deserialize)]
pub struct UpdateEntityRequest {
    pub data: Value,
}

#[derive(Deserialize)]
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
            page: params.page.into(),
        })
    }
}

#[derive(Serialize)]
pub struct ReindexResponse {
    /// The job ID assigned by the queue provider.
    pub job_id: String,
}
