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
pub(crate) struct MarketplaceTemplateArgs {
    template_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MarketplaceVersionArgs {
    template_id: Uuid,
    version_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublishArgs {
    template_id: Uuid,
    definition: serde_json::Value,
    changelog: Option<String>,
    /// One of draft, pre or stable. Defaults to draft.
    #[schemars(with = "Option<marketplace::TemplateVersionStatus>")]
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
    #[schemars(with = "crate::models::template_templates::TemplateVisibility")]
    visibility: String,
}

#[tool_router(vis = "pub(crate)", router = tool_router_marketplace)]
impl YorishiroMcpServer {
    #[tool(description = "Read one listed marketplace template")]
    pub(crate) async fn get_marketplace_template(
        &self,
        Parameters(input): Parameters<MarketplaceTemplateArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            marketplace::get_template_as(&self.app_context().db, &actor, input.template_id).await,
        )
    }

    #[tool(description = "Read one visible marketplace template version")]
    pub(crate) async fn get_marketplace_version(
        &self,
        Parameters(input): Parameters<MarketplaceVersionArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            marketplace::get_version_as(
                &self.app_context().db,
                &actor,
                input.template_id,
                input.version_id,
            )
            .await,
        )
    }
    #[tool(
        description = "List community-visible marketplace templates (requires an active licence, PostgreSQL, the marketplace:ListMarketplaceTemplates action)"
    )]
    pub(crate) async fn list_marketplace_templates(
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
        description = "List published template versions and the current tenant's own drafts (requires an active licence, PostgreSQL, the marketplace:ListMarketplaceVersions action)"
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
        description = "Publish a version of a current-tenant template (requires an active licence, the marketplace:PublishMarketplaceVersion action)"
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
        description = "List a marketplace template's reviews (requires an active licence, PostgreSQL, the marketplace:ListMarketplaceReviews action)"
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
        description = "Create or replace the current tenant's template review (requires an active licence, the marketplace:CreateMarketplaceReview action)"
    )]
    pub(crate) async fn create_marketplace_review(
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
        description = "Fork a published marketplace version into the current tenant's library (requires an active licence, the marketplace:ForkMarketplaceTemplate action)"
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
        description = "Change a current-tenant template's marketplace visibility (requires an active licence, the marketplace:UpdateMarketplaceVisibility action)"
    )]
    pub(crate) async fn update_marketplace_visibility(
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
