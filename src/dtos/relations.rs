use crate::models::entity_relations::{self, RelationStatus};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CreateRelationRequest {
    pub source_id: Uuid,
    pub target_id: Uuid,
    #[expect(clippy::doc_markdown, reason = "MCP exposes this description verbatim")]
    /// relation_type name declared in the schema's `relation_types` definition.
    pub relation_type: String,
    /// Arbitrary properties attached to the relation (JSON object, defaults to an empty object if omitted).
    pub properties: Option<Value>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ListRelationsParams {
    pub source_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub relation_type: Option<String>,
    /// Restricts the listing to one state.
    /// Omitted, every state is listed.
    pub status: Option<RelationStatus>,
    #[serde(flatten)]
    pub page: crate::dtos::common::PageParams,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SetRelationStatusRequest {
    /// `active`, `deprecated` or `archived`.
    /// Traversal follows `active` relations only.
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

impl TryFrom<ListRelationsParams> for entity_relations::ListRelationsQuery {
    type Error = crate::YorishiroError;

    fn try_from(params: ListRelationsParams) -> Result<Self, Self::Error> {
        Ok(Self {
            source_id: params.source_id,
            target_id: params.target_id,
            relation_type: params.relation_type,
            status: params.status,
            page: params.page.try_into()?,
        })
    }
}
