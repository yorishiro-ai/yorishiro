//! The template marketplace: templates shared between tenants, their published versions, and what other tenants thought of them.
//!
//! These routes gate on the licence: no key means `GET /api/marketplace` and its siblings answer 404, the same way an unlicensed deployment cannot reach `/api/stripe`.

use crate::controllers::ApiError;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ee::models::marketplace::{
    self as marketplace_models, MarketplaceListing, PublishVersionRequest, SubmitReviewRequest,
    TemplateReviewRecord, TemplateVersionRecord,
};
use crate::ee::services::authz;
use crate::ee::services::marketplace;

/// Authentication for every route in this module.
///
/// The licence check is not here: `app::licence_gate` covers this module's whole route group and
/// runs before any handler, so an unlicensed deployment answers the same `404` to everyone rather
/// than 401ing and thereby telling an anonymous prober that the endpoint exists and is merely
/// locked. That ordering holds by construction rather than by every handler remembering to check.
async fn licensed_tenant(
    ctx: &AppContext,
    headers: &HeaderMap,
) -> Result<(Uuid, Option<Uuid>), crate::YorishiroError> {
    authz::authenticate_tenant(ctx, headers).await
}

/// `GET /api/marketplace`: community-visible templates from every tenant, ordered by name then id.
async fn list_marketplace(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(page): Query<crate::controllers::PageParams>,
) -> Result<Json<Vec<MarketplaceListing>>, ApiError> {
    // The listing spans every tenant, so the identity is not read, but a valid key is still required, which is what authenticating here enforces.
    let _ = licensed_tenant(&ctx, &headers).await?;
    let listings = marketplace_models::list_marketplace(&ctx.db, page.into()).await?;
    Ok(Json(listings))
}

/// `GET /api/marketplace/{id}/versions`: published versions, plus the caller's own drafts when it owns the template.
async fn list_versions(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Query(page): Query<crate::controllers::PageParams>,
) -> Result<Json<Vec<TemplateVersionRecord>>, ApiError> {
    let (tenant_id, _) = licensed_tenant(&ctx, &headers).await?;
    let versions =
        marketplace_models::list_versions(&ctx.db, tenant_id, template_id, page.into()).await?;
    Ok(Json(versions))
}

/// `POST /api/marketplace/{id}/versions`: publish the next version of your own template.
async fn publish_version(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Json(body): Json<PublishVersionRequest>,
) -> Result<(StatusCode, Json<TemplateVersionRecord>), ApiError> {
    let (tenant_id, user_id) = licensed_tenant(&ctx, &headers).await?;
    let record = marketplace::publish_version(&ctx, tenant_id, template_id, user_id, body).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

/// `GET /api/marketplace/{id}/reviews`
async fn list_reviews(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Query(page): Query<crate::controllers::PageParams>,
) -> Result<Json<Vec<TemplateReviewRecord>>, ApiError> {
    let (tenant_id, _) = licensed_tenant(&ctx, &headers).await?;
    let reviews =
        marketplace_models::list_reviews(&ctx.db, tenant_id, template_id, page.into()).await?;
    Ok(Json(reviews))
}

/// `POST /api/marketplace/{id}/reviews`: leave or replace this tenant's review.
async fn submit_review(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Json(body): Json<SubmitReviewRequest>,
) -> Result<Json<TemplateReviewRecord>, ApiError> {
    let (tenant_id, user_id) = licensed_tenant(&ctx, &headers).await?;
    let record = marketplace::submit_review(&ctx, tenant_id, template_id, user_id, body).await?;
    Ok(Json(record))
}

#[derive(Debug, Deserialize)]
pub struct ForkParams {
    /// Which published version to copy.
    /// Omitted takes the latest `stable` one.
    pub version: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct ForkResponse {
    /// The new template in the caller's own library.
    pub template_id: Uuid,
}

/// `POST /api/marketplace/{id}/fork`: copy a published version into your own library.
async fn fork_template(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Query(params): Query<ForkParams>,
) -> Result<(StatusCode, Json<ForkResponse>), ApiError> {
    let (tenant_id, user_id) = licensed_tenant(&ctx, &headers).await?;
    let forked =
        marketplace::fork_template(&ctx, tenant_id, template_id, params.version, user_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(ForkResponse {
            template_id: forked,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct SetVisibilityRequest {
    /// `tenant` to keep it private, `community` to list it in the marketplace.
    pub visibility: String,
}

/// `PUT /api/marketplace/{id}/visibility`: list your own template, or take it back down.
async fn set_visibility(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Json(body): Json<SetVisibilityRequest>,
) -> Result<StatusCode, ApiError> {
    let (tenant_id, _) = licensed_tenant(&ctx, &headers).await?;
    marketplace::set_visibility(&ctx, tenant_id, template_id, &body.visibility).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/marketplace")
        .add("/", axum::routing::get(list_marketplace))
        .add(
            "/{id}/versions",
            axum::routing::get(list_versions).post(publish_version),
        )
        .add(
            "/{id}/reviews",
            axum::routing::get(list_reviews).post(submit_review),
        )
        .add("/{id}/fork", axum::routing::post(fork_template))
        .add("/{id}/visibility", axum::routing::put(set_visibility))
}
