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

use super::{YorishiroMcpServer, authorized, mcp_try, ok_json};
use crate::models::content_relations;
use crate::services::auth::ApiKeyScope;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateRelationArgs {
    pub source_id: Uuid,
    pub target_id: Uuid,
    /// relation_type name declared in the schema's `relation_types` definition.
    pub relation_type: String,
    /// Arbitrary properties attached to the relation (JSON object, defaults to an empty object
    /// if omitted).
    pub properties: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRelationArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteRelationArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetRelationStatusArgs {
    pub id: Uuid,
    /// "active", "deprecated" or "archived".
    /// Traversal follows "active" relations only.
    pub status: String,
}

#[tool_router(vis = "pub(crate)", router = tool_router_relations)]
impl YorishiroMcpServer {
    #[tool(
        description = "Create a relation between two entities (requires write scope). \
                           Properties cannot be edited in place; to change them, delete the \
                           relation and recreate it. To retire a relation without losing the \
                           record that it existed, use set_relation_status instead of deleting."
    )]
    pub async fn create_relation(
        &self,
        Parameters(args): Parameters<CreateRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = authorized!(&self.ctx, &parts, ApiKeyScope::Write);

        let input = content_relations::CreateRelationInput {
            source_id: args.source_id,
            target_id: args.target_id,
            relation_type: args.relation_type,
            properties: args.properties.unwrap_or_else(|| serde_json::json!({})),
        };

        let workspace_id = authorized.ctx.workspace_id;
        let record =
            mcp_try!(content_relations::create(authorized.txn(), workspace_id, input).await);
        authorized.commit().await?;
        ok_json(record)
    }

    #[tool(description = "Get a single relation by ID (requires read scope)")]
    pub async fn get_relation(
        &self,
        Parameters(args): Parameters<GetRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = authorized!(&self.ctx, &parts, ApiKeyScope::Read);

        let workspace_id = authorized.ctx.workspace_id;
        let record =
            mcp_try!(content_relations::get(authorized.txn(), workspace_id, args.id).await);
        ok_json(record)
    }

    #[tool(description = "Delete a relation (requires write scope)")]
    pub async fn delete_relation(
        &self,
        Parameters(args): Parameters<DeleteRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = authorized!(&self.ctx, &parts, ApiKeyScope::Write);

        let workspace_id = authorized.ctx.workspace_id;
        mcp_try!(content_relations::delete(authorized.txn(), workspace_id, args.id).await);
        authorized.commit().await?;
        ok_json(serde_json::json!({ "deleted": true }))
    }

    #[tool(description = "List relations (requires read scope)")]
    pub async fn list_relations(
        &self,
        Parameters(args): Parameters<ListRelationsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = authorized!(&self.ctx, &parts, ApiKeyScope::Read);

        let default = content_relations::ListRelationsQuery::default();
        let query = content_relations::ListRelationsQuery {
            source_id: args.source_id,
            target_id: args.target_id,
            relation_type: args.relation_type,
            status: args.status,
            limit: args.limit.unwrap_or(default.limit),
            offset: args.offset.unwrap_or(default.offset),
        };

        let workspace_id = authorized.ctx.workspace_id;
        let records =
            mcp_try!(content_relations::list(authorized.txn(), workspace_id, query).await);
        ok_json(records)
    }

    #[tool(
        description = "Set a relation's status to active, deprecated or archived \
                           (requires write scope). Retiring a relation this way keeps the record \
                           that it existed, which delete_relation does not; graph traversal \
                           follows active relations only."
    )]
    pub async fn set_relation_status(
        &self,
        Parameters(args): Parameters<SetRelationStatusArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = authorized!(&self.ctx, &parts, ApiKeyScope::Write);

        let workspace_id = authorized.ctx.workspace_id;
        let record = mcp_try!(
            content_relations::set_status(authorized.txn(), workspace_id, args.id, &args.status)
                .await
        );
        authorized.commit().await?;
        ok_json(record)
    }
}
