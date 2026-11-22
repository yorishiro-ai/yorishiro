//! The template marketplace: templates shared between tenants, their published versions, and what other tenants thought of them.
//!
//! These routes gate on the licence: no key means `GET /api/marketplace` and its siblings answer 404.
//! Not every route under `ee/` is gated, so this does not follow from living here or from sharing the `api/` prefix with the community edition.
//! Which routes carry the gate is decided in `app.rs`, and that is the list to read.

use crate::controllers::ApiError;
use crate::controllers::openapi::Routes;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::edition::ee::controllers::middleware::auth as authz;
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
    headers: HeaderMap,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<MarketplaceListing>>, ApiError> {
    let actor = authz::authenticate(&ctx, &headers).await?;
    let listings = marketplace_models::list_as(&ctx.db, &actor, page.try_into()?).await?;
    Ok(Json(listings))
}

/// `GET /api/marketplace/{id}/versions`: published versions, plus the caller's own drafts when it owns the template.
async fn list_versions(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(crate::dtos::common::IdPath { id: template_id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<TemplateVersionRecord>>, ApiError> {
    let actor = authz::authenticate(&ctx, &headers).await?;
    let versions =
        marketplace_models::versions_as(&ctx.db, &actor, template_id, page.try_into()?).await?;
    Ok(Json(versions))
}

/// `POST /api/marketplace/{id}/versions`: publish the next version of your own template.
async fn publish_version(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(crate::dtos::common::IdPath { id: template_id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Json(body): Json<PublishVersionInput>,
) -> Result<(StatusCode, Json<TemplateVersionRecord>), ApiError> {
    let actor = authz::authenticate(&ctx, &headers).await?;
    let request = PublishVersionRequest {
        definition: body.definition,
        changelog: body.changelog,
        status: TemplateVersionStatus::parse_publish(&body.status)?,
    };
    let record = marketplace::publish_version(&ctx, &actor, template_id, request).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

/// `GET /api/marketplace/{id}/reviews`
async fn list_reviews(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(crate::dtos::common::IdPath { id: template_id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<TemplateReviewRecord>>, ApiError> {
    let actor = authz::authenticate(&ctx, &headers).await?;
    let reviews =
        marketplace_models::reviews_as(&ctx.db, &actor, template_id, page.try_into()?).await?;
    Ok(Json(reviews))
}

/// `POST /api/marketplace/{id}/reviews`: leave or replace this tenant's review.
async fn submit_review(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(crate::dtos::common::IdPath { id: template_id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Json(body): Json<SubmitReviewRequest>,
) -> Result<Json<TemplateReviewRecord>, ApiError> {
    let actor = authz::authenticate(&ctx, &headers).await?;
    let record = marketplace::submit_review(&ctx, &actor, template_id, body).await?;
    Ok(Json(record))
}

/// `POST /api/marketplace/{id}/fork`: copy a published version into your own library.
async fn fork_template(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(crate::dtos::common::IdPath { id: template_id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Query(params): Query<ForkParams>,
) -> Result<(StatusCode, Json<ForkResponse>), ApiError> {
    let actor = authz::authenticate(&ctx, &headers).await?;
    let forked = marketplace::fork_template(&ctx, &actor, template_id, params.version).await?;
    Ok((
        StatusCode::CREATED,
        Json(ForkResponse {
            template_id: forked,
        }),
    ))
}

/// `PUT /api/marketplace/{id}/visibility`: list your own template, or take it back down.
async fn set_visibility(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(crate::dtos::common::IdPath { id: template_id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Json(body): Json<SetVisibilityRequest>,
) -> Result<StatusCode, ApiError> {
    let actor = authz::authenticate(&ctx, &headers).await?;
    let visibility = TemplateVisibility::parse_input(&body.visibility)?;
    marketplace::set_visibility(&ctx, &actor, template_id, visibility).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep route registration and aide metadata together"
)]
pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/marketplace")
        .add(
            "/",
            aide::axum::routing::get_with(list_marketplace, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["marketplace_list"]),
                    ))
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}/versions",
            aide::axum::routing::get_with(list_versions, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["marketplace_version_list"]),
                    ))
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}/versions",
            aide::axum::routing::post_with(publish_version, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["marketplace_publish"]),
                    ))
                    .response::<201, Json<TemplateVersionRecord>>()
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<422, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}/reviews",
            aide::axum::routing::get_with(list_reviews, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["marketplace_review_list"]),
                    ))
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}/reviews",
            aide::axum::routing::post_with(submit_review, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["marketplace_review_submit"]),
                    ))
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<422, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}/fork",
            aide::axum::routing::post_with(fork_template, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["marketplace_fork"]),
                    ))
                    .response::<201, Json<ForkResponse>>()
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}/visibility",
            aide::axum::routing::put_with(set_visibility, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["marketplace_visibility_update"]),
                    ))
                    .response_with::<204, (), _>(|op| {
                        op.description("Marketplace visibility updated")
                    })
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<422, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
}
