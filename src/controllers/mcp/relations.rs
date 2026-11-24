//! MCP tools for entity relations: create, get, list, delete and set status.

use crate::controllers::mcp::{render, try_tool, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use super::{YorishiroMcpServer, ok_json};
use crate::dtos::relations::CreateRelationRequest;
use crate::error::{ValidationDetail, ValidationErrorCode, YorishiroError};
use crate::models::api_keys::ApiKeyScope;
use crate::models::entity_relations::{self, RelationStatus, SetRelationStatusInput};

#[derive(Deserialize, JsonSchema)]
pub(crate) struct GetRelationArgs {
    pub id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct DeleteRelationArgs {
    pub id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListRelationsArgs {
    pub source_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub relation_type: Option<String>,
    /// Restricts the listing to one state ("active", "deprecated" or "archived").
    /// Omitted, every state is listed.
    #[schemars(with = "Option<RelationStatus>")]
    pub status: Option<String>,
    /// Maximum number of results (defaults to 50 if omitted).
    pub limit: Option<i64>,
    /// Number of records to skip (defaults to 0 if omitted).
    pub offset: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct SetRelationStatusArgs {
    pub id: Uuid,
    #[schemars(with = "RelationStatus")]
    pub status: String,
}

// Validate inside the tool to keep invalid statuses in the structured tool-error envelope.
fn parse_status(value: &str) -> Result<RelationStatus, YorishiroError> {
    RelationStatus::from_db_str(value).ok_or_else(|| YorishiroError::ValidationFailed {
        message: format!("'{value}' is not a relation status"),
        details: vec![ValidationDetail {
            field: "/status".into(),
            problem: "expected active, deprecated, or archived".into(),
            code: ValidationErrorCode::Other,
            expected: Some("active, deprecated, archived".into()),
            actual: Some(value.into()),
        }],
        hint: "use active, deprecated, or archived".into(),
    })
}

impl TryFrom<ListRelationsArgs> for entity_relations::ListRelationsQuery {
    type Error = YorishiroError;

    fn try_from(args: ListRelationsArgs) -> Result<Self, Self::Error> {
        let page = crate::models::pagination::ListParams::from(&args);
        Ok(Self {
            source_id: args.source_id,
            target_id: args.target_id,
            relation_type: args.relation_type,
            status: args.status.as_deref().map(parse_status).transpose()?,
            page,
        })
    }
}

#[tool_router(vis = "pub(crate)", router = tool_router_relations)]
impl YorishiroMcpServer {
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write),
        description = "Create a relation between two entities (requires write scope and relation_create IAM permission). \
                           Properties cannot be edited in place; to change them, delete the \
                           relation and recreate it. To retire a relation without losing the \
                           record that it existed, use set_relation_status instead of deleting."
    )]
    pub async fn create_relation(
        &self,
        Parameters(args): Parameters<CreateRelationRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "create_relation");
        render(entity_relations::create_for_actor(self.app_context(), actor, args.into()).await)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),description = "Get a single relation by ID (requires read scope and relation_get IAM permission)")]
    pub async fn get_relation(
        &self,
        Parameters(args): Parameters<GetRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "get_relation");
        render(entity_relations::get_for_actor(self.app_context(), actor, args.id).await)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write),description = "Delete a relation (requires write scope and relation_delete IAM permission)")]
    pub(crate) async fn delete_relation(
        &self,
        Parameters(args): Parameters<DeleteRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "delete_relation");
        try_tool!(entity_relations::delete_for_actor(self.app_context(), actor, args.id).await);
        ok_json(serde_json::json!({ "deleted": true }))
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),description = "List relations (requires read scope and relation_list IAM permission)")]
    pub async fn list_relations(
        &self,
        Parameters(args): Parameters<ListRelationsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "list_relations");

        let query = try_tool!(args.try_into());

        let records =
            try_tool!(entity_relations::list_for_actor(self.app_context(), &actor, query).await);
        ok_json(records)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write),
        description = "Set a relation's status to active, deprecated or archived \
                           (requires write scope and relation_update IAM permission). Retiring a relation this way keeps the record \
                           that it existed, which delete_relation does not; graph traversal \
                           follows active relations only."
    )]
    pub async fn set_relation_status(
        &self,
        Parameters(args): Parameters<SetRelationStatusArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "set_relation_status");
        render(
            entity_relations::set_status_for_actor(
                self.app_context(),
                actor,
                SetRelationStatusInput {
                    id: args.id,
                    status: try_tool!(parse_status(&args.status)),
                },
            )
            .await,
        )
    }
}

crate::dtos::common::list_params_from!(ListRelationsArgs);
