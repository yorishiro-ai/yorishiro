//! MCP tools for schema upstream-change detection and merge.
//!
//! Exposes the three origin operations: list pending upstream changes, preview a merge,
//! and apply the merge, so MCP clients can discover and act on template updates.

use crate::controllers::mcp::{YorishiroMcpServer, ok_json, render, try_tool, verified};
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

#[derive(Deserialize, JsonSchema)]
pub(crate) struct ListUpstreamChangesArgs {
    /// Maximum number of results (defaults to 50 if omitted).
    pub(crate) limit: Option<i64>,
    /// Number of records to skip (defaults to 0 if omitted).
    pub(crate) offset: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct MergePreviewArgs {
    /// ID of the schema to preview a merge for.
    pub(crate) schema_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct MergeApplyArgs {
    /// ID of the schema to merge.
    pub(crate) schema_id: Uuid,
}

#[tool_router(vis = "pub(crate)", router = tool_router_origin)]
impl YorishiroMcpServer {
    #[tool(
        description = "List schemas in this workspace whose origin template has changed since the schema was last copied (requires schema_upstream_list IAM permission). Use this to discover which schemas have pending upstream updates that have not yet been merged."
    )]
    pub(crate) async fn list_upstream_changes(
        &self,
        Parameters(args): Parameters<ListUpstreamChangesArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let page = crate::models::pagination::ListParams::from(&args);
        let changes = try_tool!(
            crate::edition::ee::models::schema_schemas::list_for_actor(
                self.app_context(),
                &actor,
                page,
            )
            .await
        );
        ok_json(changes)
    }

    #[tool(
        description = "Preview what merging the origin template would do to a schema (requires schema_merge_preview IAM permission). Returns a list of fields that would be added, updated, kept, or conflicted. Does not write anything."
    )]
    pub(crate) async fn merge_preview(
        &self,
        Parameters(args): Parameters<MergePreviewArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            crate::edition::ee::models::schema_schemas::merge_preview_for_actor(
                self.app_context(),
                actor,
                args.schema_id,
            )
            .await,
        )
    }

    #[tool(
        description = "Merge the origin template into a schema, writing the merged definition as the schema's next version (requires schema_merge_apply IAM permission). Returns the new schema version and a diff describing whether the merge was breaking. Fails if there are merge conflicts."
    )]
    pub(crate) async fn merge_apply(
        &self,
        Parameters(args): Parameters<MergeApplyArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            crate::edition::ee::models::schema_schemas::merge_apply_for_actor(
                self.app_context(),
                actor,
                args.schema_id,
            )
            .await,
        )
    }
}

crate::dtos::common::list_params_from!(ListUpstreamChangesArgs);
