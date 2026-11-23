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

use crate::edition::ee::dtos::marketplace::{
    ForkParams, ForkResponse, PublishVersionRequest as PublishVersionInput, SetVisibilityRequest,
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

/// `GET /api/marketplace/{id}/versions`: published versions, plus the caller's own drafts when it owns the template.
async fn list_versions(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdPath { id: template_id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<TemplateVersionRecord>>, ApiError> {
    let versions =
        marketplace_models::versions_as(&ctx.db, &actor, template_id, page.try_into()?).await?;
    Ok(Json(versions))
}

/// `POST /api/marketplace/{id}/versions`: publish the next version of your own template.
async fn publish_version(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdPath { id: template_id }): Path<crate::dtos::common::IdPath<Uuid>>,
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

/// `GET /api/marketplace/{id}/reviews`
async fn list_reviews(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdPath { id: template_id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<TemplateReviewRecord>>, ApiError> {
    let reviews =
        marketplace_models::reviews_as(&ctx.db, &actor, template_id, page.try_into()?).await?;
    Ok(Json(reviews))
}

/// `POST /api/marketplace/{id}/reviews`: leave or replace this tenant's review.
async fn submit_review(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdPath { id: template_id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Json(body): Json<SubmitReviewRequest>,
) -> Result<Json<TemplateReviewRecord>, ApiError> {
    let record = marketplace::submit_review(&ctx, &actor, template_id, body).await?;
    Ok(Json(record))
}

/// `POST /api/marketplace/{id}/fork`: copy a published version into your own library.
async fn fork_template(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdPath { id: template_id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Query(params): Query<ForkParams>,
) -> Result<Created<ForkResponse>, ApiError> {
    let forked = marketplace::fork_template(&ctx, &actor, template_id, params.version).await?;
    Ok(Created(ForkResponse {
        template_id: forked,
    }))
}

/// `PUT /api/marketplace/{id}/visibility`: list your own template, or take it back down.
async fn set_visibility(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdPath { id: template_id }): Path<crate::dtos::common::IdPath<Uuid>>,
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
            aide::axum::routing::get_with(list_marketplace, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["marketplace_list"]),
                ))
            }),
        )
        .add(
            "/{id}/versions",
            aide::axum::routing::get_with(list_versions, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["marketplace_version_list"]),
                ))
            }),
        )
        .add(
            "/{id}/versions",
            aide::axum::routing::post_with(publish_version, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["marketplace_publish"]),
                ))
            }),
        )
        .add(
            "/{id}/reviews",
            aide::axum::routing::get_with(list_reviews, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["marketplace_review_list"]),
                ))
            }),
        )
        .add(
            "/{id}/reviews",
            aide::axum::routing::post_with(submit_review, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["marketplace_review_submit"]),
                ))
            }),
        )
        .add(
            "/{id}/fork",
            aide::axum::routing::post_with(fork_template, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["marketplace_fork"]),
                ))
            }),
        )
        .add(
            "/{id}/visibility",
            aide::axum::routing::put_with(set_visibility, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["marketplace_visibility_update"]),
                ))
            }),
        )
}
