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
use yorishiro_core::repositories::entities;
use yorishiro_core::services::auth::ApiKeyScope;

use super::{YorishiroMcpServer, authorized, mcp_try, ok_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateEntityArgs {
    /// Name of the schema this entity conforms to. The tenant's current active version is used.
    pub schema_name: String,
    /// entity_type name declared in the schema.
    pub entity_type: String,
    /// Entity body (JSON object) conforming to the schema's `fields` definition.
    pub data: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetEntityArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateEntityArgs {
    pub id: Uuid,
    /// Replacement entity body. Validated against the schema version in effect when the entity was created.
    pub data: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteEntityArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListEntitiesArgs {
    pub entity_type: Option<String>,
    /// JSONB containment filter matched against entity data, e.g. `{"status": "active"}`.
    pub filter: Option<Value>,
    /// Restricts results to entities created against this schema version.
    pub schema_version: Option<i32>,
    /// Maximum number of results (defaults to 50 if omitted).
    pub limit: Option<i64>,
    /// Number of records to skip (defaults to 0 if omitted).
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MigrationDryRunArgs {
    /// The schema name whose entities would be migrated.
    pub name: String,
}

#[tool_router(vis = "pub(crate)", router = tool_router_entities)]
impl YorishiroMcpServer {
    #[tool(description = "Create a new entity (requires write scope)")]
    pub async fn create_entity(
        &self,
        Parameters(args): Parameters<CreateEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Write);

        let input = entities::CreateEntityInput {
            schema_name: args.schema_name,
            entity_type: args.entity_type,
            data: args.data,
        };

        let workspace_id = authorized.ctx.workspace_id;
        let created_by = authorized.ctx.user_id;
        let record =
            mcp_try!(entities::create(authorized.conn(), workspace_id, input, created_by).await);
        self.state
            .spawn_embedding_sync(authorized.ctx.tenant_id, workspace_id, record.clone());
        ok_json(record)
    }

    #[tool(description = "Get a single entity by ID (requires read scope)")]
    pub async fn get_entity(
        &self,
        Parameters(args): Parameters<GetEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Read);

        let workspace_id = authorized.ctx.workspace_id;
        let record = mcp_try!(entities::get(authorized.conn(), workspace_id, args.id).await);
        ok_json(record)
    }

    #[tool(description = "Replace the data of an existing entity (requires write scope)")]
    pub async fn update_entity(
        &self,
        Parameters(args): Parameters<UpdateEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Write);

        let workspace_id = authorized.ctx.workspace_id;
        let updated_by = authorized.ctx.user_id;
        let record = mcp_try!(
            entities::update(
                authorized.conn(),
                workspace_id,
                args.id,
                args.data,
                updated_by,
            )
            .await
        );
        self.state
            .spawn_embedding_sync(authorized.ctx.tenant_id, workspace_id, record.clone());
        ok_json(record)
    }

    #[tool(description = "Delete an entity (requires write scope)")]
    pub async fn delete_entity(
        &self,
        Parameters(args): Parameters<DeleteEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Write);

        let workspace_id = authorized.ctx.workspace_id;
        mcp_try!(entities::delete(authorized.conn(), workspace_id, args.id).await);
        ok_json(serde_json::json!({ "deleted": true }))
    }

    #[tool(description = "List entities (requires read scope)")]
    pub async fn list_entities(
        &self,
        Parameters(args): Parameters<ListEntitiesArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Read);

        let default = entities::ListEntitiesQuery::default();
        let query = entities::ListEntitiesQuery {
            entity_type: args.entity_type,
            filter: args.filter,
            schema_version: args.schema_version,
            limit: args.limit.unwrap_or(default.limit),
            offset: args.offset.unwrap_or(default.offset),
        };

        let workspace_id = authorized.ctx.workspace_id;
        let records = mcp_try!(entities::list(authorized.conn(), workspace_id, query).await);
        ok_json(records)
    }

    #[tool(
        description = "Report how an entity stands against the active version of its schema \
                           (requires read scope). Entities are migrated lazily, so one written \
                           against an older version simply lacks fields added since. Use this to \
                           tell an absent field apart from an unfilled one before answering from \
                           the entity's data."
    )]
    pub async fn get_entity_drift(
        &self,
        Parameters(args): Parameters<GetEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Read);

        let workspace_id = authorized.ctx.workspace_id;
        let drift = mcp_try!(entities::drift(authorized.conn(), workspace_id, args.id).await);
        ok_json(drift)
    }

    #[tool(
        description = "Count what migrating a schema's entities to its active version would \
                           face, without doing it (requires read scope). Reports how many are \
                           current, how many are behind but still valid, and how many lack a \
                           field the active version requires — the last being the work a \
                           migration would have to fill in."
    )]
    pub async fn migration_dry_run(
        &self,
        Parameters(args): Parameters<MigrationDryRunArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Read);

        let workspace_id = authorized.ctx.workspace_id;
        let report = mcp_try!(
            entities::migration_dry_run(authorized.conn(), workspace_id, &args.name).await
        );
        ok_json(report)
    }
}

#[cfg(test)]
#[path = "../../../tests/http/mcp/entities.rs"]
mod tests;
