//! MCP tools for the template marketplace: browsing, publishing, reviewing, forking and setting visibility.
//! These tools are licence-gated like their REST routes.

use crate::controllers::mcp::{
    YorishiroMcpServer, err_to_tool_result, ok_json, render, try_tool, verified,
};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use crate::edition::ee::models::marketplace;

pub(super) const TOOL_NAMES: &[&str] = &[
    "list_marketplace",
    "list_marketplace_versions",
    "publish_marketplace_version",
    "list_marketplace_reviews",
    "submit_marketplace_review",
    "fork_marketplace_template",
    "set_marketplace_visibility",
];

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MarketplacePage {
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TemplatePage {
    template_id: Uuid,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublishArgs {
    template_id: Uuid,
    definition: serde_json::Value,
    changelog: Option<String>,
    /// One of draft, pre or stable. Defaults to draft.
    status: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewArgs {
    template_id: Uuid,
    rating: i16,
    comment: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForkArgs {
    template_id: Uuid,
    version: Option<i32>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct VisibilityArgs {
    template_id: Uuid,
    visibility: String,
}

#[tool_router(vis = "pub(crate)", router = tool_router_marketplace)]
impl YorishiroMcpServer {
    #[tool(
        description = "List community-visible marketplace templates (requires an active licence, PostgreSQL, marketplace_list IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn list_marketplace(
        &self,
        Parameters(input): Parameters<MarketplacePage>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            marketplace::list_as(
                &self.app_context().db,
                &actor,
                crate::models::pagination::ListParams::from(&input),
            )
            .await,
        )
    }

    #[tool(
        description = "List published template versions and the current tenant's own drafts (requires an active licence, PostgreSQL, marketplace_version_list IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn list_marketplace_versions(
        &self,
        Parameters(input): Parameters<TemplatePage>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            marketplace::versions_as(
                &self.app_context().db,
                &actor,
                input.template_id,
                crate::models::pagination::ListParams::from(&input),
            )
            .await,
        )
    }

    #[tool(
        description = "Publish a version of a current-tenant template (requires an active licence, marketplace_publish IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn publish_marketplace_version(
        &self,
        Parameters(input): Parameters<PublishArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let status = try_tool!(marketplace::TemplateVersionStatus::parse_publish(
            input.status.as_deref().unwrap_or("draft"),
        ));
        render(
            marketplace::publish_version(
                self.app_context(),
                &actor,
                input.template_id,
                marketplace::PublishVersionRequest {
                    definition: input.definition,
                    changelog: input.changelog,
                    status,
                },
            )
            .await,
        )
    }

    #[tool(
        description = "List a marketplace template's reviews (requires an active licence, PostgreSQL, marketplace_review_list IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn list_marketplace_reviews(
        &self,
        Parameters(input): Parameters<TemplatePage>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            marketplace::reviews_as(
                &self.app_context().db,
                &actor,
                input.template_id,
                crate::models::pagination::ListParams::from(&input),
            )
            .await,
        )
    }

    #[tool(
        description = "Create or replace the current tenant's template review (requires an active licence, marketplace_review_submit IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn submit_marketplace_review(
        &self,
        Parameters(input): Parameters<ReviewArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            marketplace::submit_review(
                self.app_context(),
                &actor,
                input.template_id,
                marketplace::SubmitReviewRequest {
                    rating: input.rating,
                    comment: input.comment,
                },
            )
            .await,
        )
    }

    #[tool(
        description = "Fork a published marketplace version into the current tenant's library (requires an active licence, marketplace_fork IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn fork_marketplace_template(
        &self,
        Parameters(input): Parameters<ForkArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        match marketplace::fork_template(
            self.app_context(),
            &actor,
            input.template_id,
            input.version,
        )
        .await
        {
            Ok(template_id) => ok_json(serde_json::json!({"template_id": template_id})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "Change a current-tenant template's marketplace visibility (requires an active licence, marketplace_visibility_update IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn set_marketplace_visibility(
        &self,
        Parameters(input): Parameters<VisibilityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let visibility = try_tool!(
            crate::models::template_templates::TemplateVisibility::parse_input(&input.visibility,)
        );
        match marketplace::set_visibility(self.app_context(), &actor, input.template_id, visibility)
            .await
        {
            Ok(()) => ok_json(serde_json::json!({"updated": true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}

crate::dtos::common::list_params_from!(MarketplacePage, TemplatePage);
