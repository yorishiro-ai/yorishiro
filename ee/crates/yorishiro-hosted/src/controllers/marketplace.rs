//! The template marketplace: templates shared between tenants, their published versions, and what other tenants thought of them.
//!
//! These routes gate on the licence: no key means `GET /hosted/marketplace` and its siblings answer 404, the same way an unlicensed deployment cannot reach `/hosted/stripe`.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use yorishiro_core::controllers::ApiError;

use crate::models::marketplace::{
    self as marketplace_models, MarketplaceListing, PublishVersionRequest, SubmitReviewRequest,
    TemplateReviewRecord, TemplateVersionRecord,
};
use crate::services::authz;
use crate::services::licence::LicenceState;
use crate::services::marketplace;

/// The licence gate for every route in this module, paired with authentication so the two cannot drift apart: a handler added here reaches for this rather than `authz::authenticate_tenant` directly, and is gated by construction.
///
/// Checked *before* authentication, so an unlicensed deployment answers the same `404` whether or not the caller holds a valid key: a marketplace that 401s tells an anonymous prober that it exists here and is merely locked.
async fn licensed_tenant(
    ctx: &AppContext,
    headers: &HeaderMap,
) -> Result<(Uuid, Option<Uuid>), yorishiro_core::YorishiroError> {
    ctx.shared_store
        .get::<LicenceState>()
        .ok_or_else(|| {
            yorishiro_core::YorishiroError::Internal(anyhow::anyhow!("LicenceState missing"))
        })?
        .require_active()?;
    authz::authenticate_tenant(ctx, headers).await
}

/// `GET /hosted/marketplace`: community-visible templates from every tenant, ordered by name then id.
async fn list_marketplace(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(page): Query<yorishiro_core::controllers::PageParams>,
) -> Result<Json<Vec<MarketplaceListing>>, ApiError> {
    // The listing spans every tenant, so the identity is not read, but a valid key is still required, which is what authenticating here enforces.
    let _ = licensed_tenant(&ctx, &headers).await?;
    let listings = marketplace_models::list_marketplace(&ctx.db, page.into()).await?;
    Ok(Json(listings))
}

/// `GET /hosted/marketplace/{id}/versions`: published versions, plus the caller's own drafts when it owns the template.
async fn list_versions(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Query(page): Query<yorishiro_core::controllers::PageParams>,
) -> Result<Json<Vec<TemplateVersionRecord>>, ApiError> {
    let (tenant_id, _) = licensed_tenant(&ctx, &headers).await?;
    let versions =
        marketplace_models::list_versions(&ctx.db, tenant_id, template_id, page.into()).await?;
    Ok(Json(versions))
}

/// `POST /hosted/marketplace/{id}/versions`: publish the next version of your own template.
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

/// `GET /hosted/marketplace/{id}/reviews`
async fn list_reviews(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Query(page): Query<yorishiro_core::controllers::PageParams>,
) -> Result<Json<Vec<TemplateReviewRecord>>, ApiError> {
    let (tenant_id, _) = licensed_tenant(&ctx, &headers).await?;
    let reviews =
        marketplace_models::list_reviews(&ctx.db, tenant_id, template_id, page.into()).await?;
    Ok(Json(reviews))
}

/// `POST /hosted/marketplace/{id}/reviews`: leave or replace this tenant's review.
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

/// `POST /hosted/marketplace/{id}/fork`: copy a published version into your own library.
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

/// `PUT /hosted/marketplace/{id}/visibility`: list your own template, or take it back down.
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
        .prefix("hosted")
        .add("/marketplace", axum::routing::get(list_marketplace))
        .add(
            "/marketplace/{id}/versions",
            axum::routing::get(list_versions).post(publish_version),
        )
        .add(
            "/marketplace/{id}/reviews",
            axum::routing::get(list_reviews).post(submit_review),
        )
        .add("/marketplace/{id}/fork", axum::routing::post(fork_template))
        .add(
            "/marketplace/{id}/visibility",
            axum::routing::put(set_visibility),
        )
}
