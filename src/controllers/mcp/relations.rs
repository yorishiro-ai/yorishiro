//! MCP tools for entity relations: create, get, list, delete and set status.

use crate::controllers::mcp::{try_tool, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::{YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::models::api_keys::ApiKeyScope;
use crate::models::entity_relations::{self, SetRelationStatusInput};

#[derive(Deserialize, JsonSchema)]
pub struct CreateRelationArgs {
    pub source_id: Uuid,
    pub target_id: Uuid,
    #[expect(clippy::doc_markdown, reason = "MCP exposes this description verbatim")]
    /// relation_type name declared in the schema's `relation_types` definition.
    pub relation_type: String,
    /// Arbitrary properties attached to the relation (JSON object, defaults to an empty object if omitted).
    pub properties: Option<Value>,
}

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
    pub status: Option<String>,
    /// Maximum number of results (defaults to 50 if omitted).
    pub limit: Option<i64>,
    /// Number of records to skip (defaults to 0 if omitted).
    pub offset: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct SetRelationStatusArgs {
    pub id: Uuid,
    /// "active", "deprecated" or "archived".
    /// Traversal follows "active" relations only.
    pub status: String,
}

impl From<CreateRelationArgs> for entity_relations::CreateRelationInput {
    fn from(args: CreateRelationArgs) -> Self {
        Self {
            source_id: args.source_id,
            target_id: args.target_id,
            relation_type: args.relation_type,
            properties: args.properties.unwrap_or_else(|| serde_json::json!({})),
        }
    }
}

impl TryFrom<ListRelationsArgs> for entity_relations::ListRelationsQuery {
    type Error = crate::YorishiroError;

    fn try_from(args: ListRelationsArgs) -> Result<Self, Self::Error> {
        let status = args
            .status
            .as_deref()
            .map(entity_relations::parse_relation_status)
            .transpose()?;
        Ok(Self {
            source_id: args.source_id,
            target_id: args.target_id,
            relation_type: args.relation_type,
            status,
            page: crate::models::pagination::ListParams::new(args.limit, args.offset),
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
        Parameters(args): Parameters<CreateRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let required = self.required_scope("create_relation")?;
        let actor = verified!(super::verify(self.app_context(), &parts, required).await?);
        let mut authorized = try_tool!(
            super::authorize::Authorized::for_target(
                self.app_context(),
                &actor,
                required,
                crate::models::iam::access::WorkspaceTarget::Workspace(
                    match crate::models::entity_entities::workspace_for_actor(
                        &self.app_context().db,
                        &actor,
                        args.source_id,
                    )
                    .await
                    {
                        Ok(id) => id,
                        Err(error) => return Ok(err_to_tool_result(error)),
                    },
                ),
            )
            .await
        );
        try_tool!(
            authorized
                .require_action(crate::models::iam::policy::Action::RelationCreate)
                .await
        );

        let input = args.into();

        let workspace_id = authorized.workspace_id();
        let record =
            try_tool!(entity_relations::create(authorized.txn(), workspace_id, input).await);
        authorized.commit().await?;
        ok_json(record)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),description = "Get a single relation by ID (requires read scope and relation_get IAM permission)")]
    pub async fn get_relation(
        &self,
        Parameters(args): Parameters<GetRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let required = self.required_scope("get_relation")?;
        let actor = verified!(super::verify(self.app_context(), &parts, required).await?);
        let mut authorized = try_tool!(
            super::authorize::Authorized::for_target(
                self.app_context(),
                &actor,
                required,
                crate::models::iam::access::WorkspaceTarget::Relation(args.id),
            )
            .await
        );
        try_tool!(
            authorized
                .require_action(crate::models::iam::policy::Action::RelationGet)
                .await
        );

        let workspace_id = authorized.workspace_id();
        let record =
            try_tool!(entity_relations::get(authorized.txn(), workspace_id, args.id).await);
        ok_json(record)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write),description = "Delete a relation (requires write scope and relation_delete IAM permission)")]
    pub(crate) async fn delete_relation(
        &self,
        Parameters(args): Parameters<DeleteRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let required = self.required_scope("delete_relation")?;
        let actor = verified!(super::verify(self.app_context(), &parts, required).await?);
        let mut authorized = try_tool!(
            super::authorize::Authorized::for_target(
                self.app_context(),
                &actor,
                required,
                crate::models::iam::access::WorkspaceTarget::Relation(args.id),
            )
            .await
        );
        try_tool!(
            authorized
                .require_action(crate::models::iam::policy::Action::RelationDelete)
                .await
        );

        let workspace_id = authorized.workspace_id();
        try_tool!(entity_relations::delete(authorized.txn(), workspace_id, args.id).await);
        authorized.commit().await?;
        ok_json(serde_json::json!({ "deleted": true }))
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),description = "List relations (requires read scope and relation_list IAM permission)")]
    pub async fn list_relations(
        &self,
        Parameters(args): Parameters<ListRelationsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(
            super::verify(
                self.app_context(),
                &parts,
                self.required_scope("list_relations")?,
            )
            .await?
        );

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
        let required = self.required_scope("set_relation_status")?;
        let actor = verified!(super::verify(self.app_context(), &parts, required).await?);
        let mut authorized = try_tool!(
            super::authorize::Authorized::for_target(
                self.app_context(),
                &actor,
                required,
                crate::models::iam::access::WorkspaceTarget::Relation(args.id),
            )
            .await
        );
        try_tool!(
            authorized
                .require_action(crate::models::iam::policy::Action::RelationUpdate)
                .await
        );

        let workspace_id = authorized.workspace_id();
        let status = try_tool!(entity_relations::parse_relation_status(&args.status));
        let record = try_tool!(
            entity_relations::set_status(
                authorized.txn(),
                workspace_id,
                SetRelationStatusInput {
                    id: args.id,
                    status,
                },
            )
            .await
        );
        authorized.commit().await?;
        ok_json(record)
    }
}
