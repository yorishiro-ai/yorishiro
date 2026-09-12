//! MCP tools for schema upstream-change detection and merge.
//!
//! Exposes the three origin operations — list pending upstream changes, preview a merge,
//! and apply the merge — so MCP clients (agents) can discover and act on template updates.

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

use super::{AuthzOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::services::auth::ApiKeyScope;

#[derive(Deserialize, JsonSchema)]
pub struct ListUpstreamChangesArgs {
    /// Maximum number of results (defaults to 50 if omitted).
    pub limit: Option<i64>,
    /// Number of records to skip (defaults to 0 if omitted).
    pub offset: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MergePreviewArgs {
    /// ID of the schema to preview a merge for.
    pub schema_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub struct MergeApplyArgs {
    /// ID of the schema to merge.
    pub schema_id: Uuid,
}

#[tool_router(vis = "pub(crate)", router = tool_router_origin)]
impl YorishiroMcpServer {
    #[tool(
        description = "List schemas in this workspace whose origin template has changed since \
                       the schema was last copied (requires read scope). Use this to discover \
                       which schemas have pending upstream updates that have not yet been \
                       merged."
    )]
    pub async fn list_upstream_changes(
        &self,
        Parameters(args): Parameters<ListUpstreamChangesArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(&self.ctx, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let workspace_id = authorized.ctx.workspace_id;
        let page = crate::models::pagination::ListParams::new(args.limit, args.offset);
        let changes = match crate::ee::models::origin::list_with_upstream_changes(
            &self.ctx.db,
            workspace_id,
            page,
        )
        .await
        {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        ok_json(changes)
    }

    #[tool(
        description = "Preview what merging the origin template would do to a schema (requires \
                       read scope). Returns a list of fields that would be added, updated, \
                       kept, or conflicted. Does not write anything."
    )]
    pub async fn merge_preview(
        &self,
        Parameters(args): Parameters<MergePreviewArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(&self.ctx, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let db = crate::controllers::extractors::db_handle(&self.ctx)
            .map_err(|err| ErrorData::internal_error(err.0.to_string(), None))?;
        let schema_txn = db
            .tenant
            .begin_for_workspace(authorized.ctx.tenant_id, authorized.ctx.workspace_id)
            .await
            .map_err(|err| ErrorData::internal_error(err.to_string(), None))?;

        let plan = match crate::ee::services::origin::merge_preview(
            &schema_txn,
            &self.ctx,
            authorized.ctx.tenant_id,
            authorized.ctx.workspace_id,
            args.schema_id,
        )
        .await
        {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        // Read-only: rolling back a transaction that made no writes is equivalent.
        ok_json(plan)
    }

    #[tool(
        description = "Merge the origin template into a schema, writing the merged definition \
                       as the schema's next version (requires schema scope). Returns the new \
                       schema version and a diff describing whether the merge was breaking. \
                       Fails if there are merge conflicts."
    )]
    pub async fn merge_apply(
        &self,
        Parameters(args): Parameters<MergeApplyArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(&self.ctx, &parts, ApiKeyScope::Schema).await? {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let db = crate::controllers::extractors::db_handle(&self.ctx)
            .map_err(|err| ErrorData::internal_error(err.0.to_string(), None))?;
        let schema_txn = db
            .tenant
            .begin_for_workspace(authorized.ctx.tenant_id, authorized.ctx.workspace_id)
            .await
            .map_err(|err| ErrorData::internal_error(err.to_string(), None))?;

        let (schema, diff) = match crate::ee::services::origin::merge_apply(
            &schema_txn,
            &self.ctx,
            authorized.ctx.tenant_id,
            authorized.ctx.workspace_id,
            args.schema_id,
        )
        .await
        {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        schema_txn
            .commit()
            .await
            .map_err(|err| ErrorData::internal_error(err.to_string(), None))?;
        ok_json(serde_json::json!({
            "schema": schema,
            "diff": diff,
        }))
    }
}
