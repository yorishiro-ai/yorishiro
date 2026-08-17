//! The template marketplace: templates shared between tenants, their published versions, and
//! what other tenants thought of them.
//!
//! Distribution between tenants is an enterprise capability, so these routes live here rather
//! than in the community edition.
//!
//! Authentication differs from the community edition's version by necessity, not by design.
//! There, handlers took `Authorized<ReadScope>` from `yorishiro-server`'s middleware; this
//! crate's lib may not depend on that crate, so they resolve the key through
//! [`authz::authenticate_tenant`] instead. The rule is the same one every route in this
//! process follows: any valid key for the tenant, with ownership enforced by the service
//! rather than by the role.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::error::HostedApiError;
use crate::services::authz;
use crate::services::marketplace::{
    self, MarketplaceListing, PublishVersionRequest, SubmitReviewRequest, TemplateReviewRecord,
    TemplateVersionRecord,
};
use crate::state::HostedState;

/// The licence gate for every route in this module, paired with authentication so the two cannot
/// drift apart -- a handler added here reaches for this rather than `authz::authenticate_tenant`
/// directly, and is gated by construction.
///
/// The gate is not on `authenticate_tenant` itself: the tenant dashboard authenticates through
/// the same function and is part of the free floor (requirements FR-5-3), so gating there would
/// close a page that must stay open.
///
/// It is checked *before* authentication, so an unlicensed deployment answers the same `404`
/// whether or not the caller holds a valid key -- a marketplace that 401s tells an anonymous
/// prober that it exists here and is merely locked.
async fn licensed_tenant(
    state: &HostedState,
    headers: &HeaderMap,
) -> Result<(Uuid, Option<Uuid>), yorishiro_core::YorishiroError> {
    state.licence.require_active()?;
    authz::authenticate_tenant(state, headers).await
}

/// `GET /api/marketplace` -- community-visible templates from every tenant.
#[utoipa::path(
    get,
    path = "/api/marketplace",
    responses(
        (status = 200, description = "Community-visible templates with their latest stable version and review aggregates", body = Vec<MarketplaceListing>),
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::HostedApiErrorBody),
    ),
    security(("bearer_key" = [])),
    tag = "marketplace",
)]
pub async fn list_marketplace(
    State(state): State<HostedState>,
    headers: HeaderMap,
) -> Result<Json<Vec<MarketplaceListing>>, HostedApiError> {
    // The listing spans every tenant, so the identity is not read -- but a valid key is still
    // required, which is what authenticating here enforces.
    let _ = licensed_tenant(&state, &headers).await?;
    let listings = marketplace::list_marketplace(&state.identity_pool).await?;
    Ok(Json(listings))
}

/// `GET /api/marketplace/{id}/versions` -- published versions, plus the caller's own drafts
/// when it owns the template.
#[utoipa::path(
    get,
    path = "/api/marketplace/{id}/versions",
    params(("id" = Uuid, Path, description = "Template ID")),
    responses(
        (status = 200, description = "Versions visible to the caller, newest first", body = Vec<TemplateVersionRecord>),
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::HostedApiErrorBody),
    ),
    security(("bearer_key" = [])),
    tag = "marketplace",
)]
pub async fn list_versions(
    State(state): State<HostedState>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
) -> Result<Json<Vec<TemplateVersionRecord>>, HostedApiError> {
    let (tenant_id, _) = licensed_tenant(&state, &headers).await?;
    let versions = marketplace::list_versions(&state.identity_pool, tenant_id, template_id).await?;
    Ok(Json(versions))
}

/// `POST /api/marketplace/{id}/versions` -- publish the next version of your own template.
#[utoipa::path(
    post,
    path = "/api/marketplace/{id}/versions",
    params(("id" = Uuid, Path, description = "Template ID")),
    request_body = PublishVersionRequest,
    responses(
        (status = 201, description = "Version published", body = TemplateVersionRecord),
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::HostedApiErrorBody),
        (status = 404, description = "No such template belonging to the caller's tenant", body = crate::error::HostedApiErrorBody),
        (status = 422, description = "Unknown publish status", body = crate::error::HostedApiErrorBody),
    ),
    security(("bearer_key" = [])),
    tag = "marketplace",
)]
pub async fn publish_version(
    State(state): State<HostedState>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Json(body): Json<PublishVersionRequest>,
) -> Result<(StatusCode, Json<TemplateVersionRecord>), HostedApiError> {
    let (tenant_id, user_id) = licensed_tenant(&state, &headers).await?;
    let record =
        marketplace::publish_version(&state.identity_pool, tenant_id, template_id, user_id, body)
            .await?;
    Ok((StatusCode::CREATED, Json(record)))
}

/// `GET /api/marketplace/{id}/reviews`
#[utoipa::path(
    get,
    path = "/api/marketplace/{id}/reviews",
    params(("id" = Uuid, Path, description = "Template ID")),
    responses(
        (status = 200, description = "Reviews of a template the caller can see", body = Vec<TemplateReviewRecord>),
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::HostedApiErrorBody),
    ),
    security(("bearer_key" = [])),
    tag = "marketplace",
)]
pub async fn list_reviews(
    State(state): State<HostedState>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
) -> Result<Json<Vec<TemplateReviewRecord>>, HostedApiError> {
    let (tenant_id, _) = licensed_tenant(&state, &headers).await?;
    let reviews = marketplace::list_reviews(&state.identity_pool, tenant_id, template_id).await?;
    Ok(Json(reviews))
}

/// `POST /api/marketplace/{id}/reviews` -- leave or replace this tenant's review.
#[utoipa::path(
    post,
    path = "/api/marketplace/{id}/reviews",
    params(("id" = Uuid, Path, description = "Template ID")),
    request_body = SubmitReviewRequest,
    responses(
        (status = 200, description = "Review recorded, replacing this tenant's previous one if any", body = TemplateReviewRecord),
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::HostedApiErrorBody),
        (status = 404, description = "No such template visible to the caller", body = crate::error::HostedApiErrorBody),
        (status = 422, description = "Rating outside 1-5", body = crate::error::HostedApiErrorBody),
    ),
    security(("bearer_key" = [])),
    tag = "marketplace",
)]
pub async fn submit_review(
    State(state): State<HostedState>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Json(body): Json<SubmitReviewRequest>,
) -> Result<Json<TemplateReviewRecord>, HostedApiError> {
    let (tenant_id, user_id) = licensed_tenant(&state, &headers).await?;
    let record =
        marketplace::submit_review(&state.identity_pool, tenant_id, template_id, user_id, body)
            .await?;
    Ok(Json(record))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ForkParams {
    /// Which published version to copy. Omitted takes the latest `stable` one.
    pub version: Option<i32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ForkResponse {
    /// The new template in the caller's own library.
    pub template_id: Uuid,
}

/// `POST /api/marketplace/{id}/fork` -- copy a published version into your own library.
#[utoipa::path(
    post,
    path = "/api/marketplace/{id}/fork",
    params(("id" = Uuid, Path, description = "Template ID"), ForkParams),
    responses(
        (status = 201, description = "Forked into the caller's tenant, private", body = ForkResponse),
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::HostedApiErrorBody),
        (status = 404, description = "No such template, or no published version to fork", body = crate::error::HostedApiErrorBody),
        (status = 409, description = "The caller's tenant already has a template of that name", body = crate::error::HostedApiErrorBody),
    ),
    security(("bearer_key" = [])),
    tag = "marketplace",
)]
pub async fn fork_template(
    State(state): State<HostedState>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Query(params): Query<ForkParams>,
) -> Result<(StatusCode, Json<ForkResponse>), HostedApiError> {
    let (tenant_id, user_id) = licensed_tenant(&state, &headers).await?;
    let forked = marketplace::fork_template(
        &state.identity_pool,
        tenant_id,
        template_id,
        params.version,
        user_id,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(ForkResponse {
            template_id: forked,
        }),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetVisibilityRequest {
    /// `tenant` to keep it private, `community` to list it in the marketplace.
    pub visibility: String,
}

/// `PUT /api/marketplace/{id}/visibility` -- list your own template, or take it back down.
#[utoipa::path(
    put,
    path = "/api/marketplace/{id}/visibility",
    params(("id" = Uuid, Path, description = "Template ID")),
    request_body = SetVisibilityRequest,
    responses(
        (status = 204, description = "Visibility updated"),
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::HostedApiErrorBody),
        (status = 404, description = "No such template belonging to the caller's tenant", body = crate::error::HostedApiErrorBody),
        (status = 422, description = "Unknown visibility", body = crate::error::HostedApiErrorBody),
    ),
    security(("bearer_key" = [])),
    tag = "marketplace",
)]
pub async fn set_visibility(
    State(state): State<HostedState>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Json(body): Json<SetVisibilityRequest>,
) -> Result<StatusCode, HostedApiError> {
    let (tenant_id, _) = licensed_tenant(&state, &headers).await?;
    marketplace::set_visibility(
        &state.identity_pool,
        tenant_id,
        template_id,
        &body.visibility,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[path = "../../../tests/http/controllers/marketplace.rs"]
mod tests;
