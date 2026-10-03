use crate::models::entity_relations::{self, RelationStatus};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateRelationRequest {
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    pub properties: Option<Value>,
}

#[derive(Deserialize)]
pub struct ListRelationsParams {
    pub source_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub relation_type: Option<String>,
    /// Restricts the listing to one state.
    /// Omitted, every state is listed.
    pub(crate) status: Option<RelationStatus>,
    #[serde(flatten)]
    pub page: crate::dtos::common::PageParams,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetRelationStatusRequest {
    /// `active`, `deprecated` or `archived`.
    pub(crate) status: RelationStatus,
}

impl From<CreateRelationRequest> for entity_relations::CreateRelationInput {
    fn from(request: CreateRelationRequest) -> Self {
        Self {
            source_id: request.source_id,
            target_id: request.target_id,
            relation_type: request.relation_type,
            properties: request.properties.unwrap_or_else(|| serde_json::json!({})),
        }
    }
}

impl From<ListRelationsParams> for entity_relations::ListRelationsQuery {
    fn from(params: ListRelationsParams) -> Self {
        Self {
            source_id: params.source_id,
            target_id: params.target_id,
            relation_type: params.relation_type,
            status: params.status,
            page: params.page.into(),
        }
    }
}
