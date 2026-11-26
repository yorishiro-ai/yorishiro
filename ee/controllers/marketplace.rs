//! The template marketplace: templates shared between tenants, their published versions, and what other tenants thought of them.
//!
//! These routes gate on the licence: no key means `GET /api/marketplace` and its siblings answer 404.
//! Not every route under `ee/` is gated, so this does not follow from living here or from sharing the `api/` prefix with the community edition.
//! Which routes carry the gate is decided in `app.rs`, and that is the list to read.

use crate::controllers::ApiError;
use crate::controllers::Created;
use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::dtos::common::TemplateIdPath;
use crate::edition::ee::dtos::marketplace::{
    ForkParams, ForkResponse, MarketplaceVersionPath, PublishVersionRequest as PublishVersionInput,
    SetVisibilityRequest,
};
use crate::edition::ee::models::marketplace;
use crate::edition::ee::models::marketplace::{
    self as marketplace_models, MarketplaceListing, PublishVersionRequest, SubmitReviewRequest,
    TemplateReviewRecord, TemplateVersionRecord, TemplateVersionStatus,
};
use crate::models::template_templates::TemplateVisibility;

/// `GET /api/marketplace`: community-visible templates from every tenant, ordered by name then id.
async fn list_marketplace(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<MarketplaceListing>>, ApiError> {
    let listings = marketplace_models::list_as(&ctx.db, &actor, page.try_into()?).await?;
    Ok(Json(listings))
}

async fn get_marketplace_template(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(TemplateIdPath { template_id }): Path<TemplateIdPath<Uuid>>,
) -> Result<Json<MarketplaceListing>, ApiError> {
    Ok(Json(
        marketplace::get_template_as(&ctx.db, &actor, template_id).await?,
    ))
}

async fn get_marketplace_version(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(MarketplaceVersionPath {
        template_id,
        version_id,
    }): Path<MarketplaceVersionPath>,
) -> Result<Json<TemplateVersionRecord>, ApiError> {
    Ok(Json(
        marketplace::get_version_as(&ctx.db, &actor, template_id, version_id).await?,
    ))
}

/// Lists published versions and includes drafts for the template owner.
async fn list_versions(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(TemplateIdPath { template_id }): Path<TemplateIdPath<Uuid>>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<TemplateVersionRecord>>, ApiError> {
    let versions =
        marketplace_models::versions_as(&ctx.db, &actor, template_id, page.try_into()?).await?;
    Ok(Json(versions))
}

/// Publishes the next version of a template the caller owns.
async fn publish_version(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(TemplateIdPath { template_id }): Path<TemplateIdPath<Uuid>>,
    Json(body): Json<PublishVersionInput>,
) -> Result<Created<TemplateVersionRecord>, ApiError> {
    let request = PublishVersionRequest {
        definition: body.definition,
        changelog: body.changelog,
        status: TemplateVersionStatus::parse_publish(&body.status)?,
    };
    let record = marketplace::publish_version(&ctx, &actor, template_id, request).await?;
    Ok(Created(record))
}

/// Lists reviews for a marketplace template.
async fn list_reviews(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(TemplateIdPath { template_id }): Path<TemplateIdPath<Uuid>>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<TemplateReviewRecord>>, ApiError> {
    let reviews =
        marketplace_models::reviews_as(&ctx.db, &actor, template_id, page.try_into()?).await?;
    Ok(Json(reviews))
}

/// Creates or replaces the current tenant's review.
async fn submit_review(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(TemplateIdPath { template_id }): Path<TemplateIdPath<Uuid>>,
    Json(body): Json<SubmitReviewRequest>,
) -> Result<Json<TemplateReviewRecord>, ApiError> {
    let record = marketplace::submit_review(&ctx, &actor, template_id, body).await?;
    Ok(Json(record))
}

/// Copies a published version into the current tenant's library.
async fn fork_template(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(TemplateIdPath { template_id }): Path<TemplateIdPath<Uuid>>,
    Query(params): Query<ForkParams>,
) -> Result<Created<ForkResponse>, ApiError> {
    let forked = marketplace::fork_template(&ctx, &actor, template_id, params.version).await?;
    Ok(Created(ForkResponse {
        template_id: forked,
    }))
}

/// Changes whether a template the caller owns appears in the marketplace.
async fn set_visibility(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(TemplateIdPath { template_id }): Path<TemplateIdPath<Uuid>>,
    Json(body): Json<SetVisibilityRequest>,
) -> Result<axum::response::NoContent, ApiError> {
    let visibility = TemplateVisibility::parse_input(&body.visibility)?;
    marketplace::set_visibility(&ctx, &actor, template_id, visibility).await?;
    Ok(axum::response::NoContent)
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("enterprise")
        .prefix("api/marketplace")
        .add(
            "/",
            aide::axum::routing::get_with(list_marketplace, |op| op.id("ListMarketplaceTemplates")),
        )
        .add(
            "/{template_id}",
            aide::axum::routing::get_with(get_marketplace_template, |op| {
                op.id("GetMarketplaceTemplate")
            }),
        )
        .add(
            "/{template_id}/versions/{version_id}",
            aide::axum::routing::get_with(get_marketplace_version, |op| {
                op.id("GetMarketplaceVersion")
            }),
        )
        .add(
            "/{template_id}/versions",
            aide::axum::routing::get_with(list_versions, |op| op.id("ListMarketplaceVersions")),
        )
        .add(
            "/{template_id}/versions",
            aide::axum::routing::post_with(publish_version, |op| {
                op.id("PublishMarketplaceVersion")
            }),
        )
        .add(
            "/{template_id}/reviews",
            aide::axum::routing::get_with(list_reviews, |op| op.id("ListMarketplaceReviews")),
        )
        .add(
            "/{template_id}/reviews",
            aide::axum::routing::post_with(submit_review, |op| op.id("CreateMarketplaceReview")),
        )
        .add(
            "/{template_id}/fork",
            aide::axum::routing::post_with(fork_template, |op| op.id("ForkMarketplaceTemplate")),
        )
        .add(
            "/{template_id}/visibility",
            aide::axum::routing::put_with(set_visibility, |op| {
                op.id("UpdateMarketplaceVisibility")
            }),
        )
}
