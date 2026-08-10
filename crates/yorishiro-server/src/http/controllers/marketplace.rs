use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use yorishiro_core::services::marketplace::{
    self, MarketplaceListing, PublishVersionRequest, SubmitReviewRequest, TemplateReviewRecord,
    TemplateVersionRecord,
};

use crate::error::ApiError;
use crate::http::middleware::auth::AuthContext;
use crate::state::AppState;

/// `GET /api/marketplace` -- community-visible templates from every tenant.
#[utoipa::path(
    get,
    path = "/api/marketplace",
    responses(
        (status = 200, description = "Community-visible templates with their latest stable version and review aggregates", body = Vec<MarketplaceListing>),
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::ApiErrorBody),
    ),
    tag = "marketplace",
)]
pub async fn list_marketplace(
    State(state): State<AppState>,
    // The listing spans every tenant, so the context is not read -- but a valid key is still
    // required, which is what naming the extractor here enforces.
    AuthContext(_ctx): AuthContext,
) -> Result<Json<Vec<MarketplaceListing>>, ApiError> {
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
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::ApiErrorBody),
    ),
    tag = "marketplace",
)]
pub async fn list_versions(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Path(template_id): Path<Uuid>,
) -> Result<Json<Vec<TemplateVersionRecord>>, ApiError> {
    let tenant_id = ctx.tenant_id;
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
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::ApiErrorBody),
        (status = 404, description = "No such template belonging to the caller's tenant", body = crate::error::ApiErrorBody),
        (status = 422, description = "Unknown publish status", body = crate::error::ApiErrorBody),
    ),
    tag = "marketplace",
)]
pub async fn publish_version(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Path(template_id): Path<Uuid>,
    Json(body): Json<PublishVersionRequest>,
) -> Result<(StatusCode, Json<TemplateVersionRecord>), ApiError> {
    let (tenant_id, user_id) = (ctx.tenant_id, ctx.user_id);
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
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::ApiErrorBody),
    ),
    tag = "marketplace",
)]
pub async fn list_reviews(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Path(template_id): Path<Uuid>,
) -> Result<Json<Vec<TemplateReviewRecord>>, ApiError> {
    let tenant_id = ctx.tenant_id;
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
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::ApiErrorBody),
        (status = 404, description = "No such template visible to the caller", body = crate::error::ApiErrorBody),
        (status = 422, description = "Rating outside 1-5", body = crate::error::ApiErrorBody),
    ),
    tag = "marketplace",
)]
pub async fn submit_review(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Path(template_id): Path<Uuid>,
    Json(body): Json<SubmitReviewRequest>,
) -> Result<Json<TemplateReviewRecord>, ApiError> {
    let (tenant_id, user_id) = (ctx.tenant_id, ctx.user_id);
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
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::ApiErrorBody),
        (status = 404, description = "No such template, or no published version to fork", body = crate::error::ApiErrorBody),
        (status = 409, description = "The caller's tenant already has a template of that name", body = crate::error::ApiErrorBody),
    ),
    tag = "marketplace",
)]
pub async fn fork_template(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Path(template_id): Path<Uuid>,
    Query(params): Query<ForkParams>,
) -> Result<(StatusCode, Json<ForkResponse>), ApiError> {
    let (tenant_id, user_id) = (ctx.tenant_id, ctx.user_id);
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
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::ApiErrorBody),
        (status = 404, description = "No such template belonging to the caller's tenant", body = crate::error::ApiErrorBody),
        (status = 422, description = "Unknown visibility", body = crate::error::ApiErrorBody),
    ),
    tag = "marketplace",
)]
pub async fn set_visibility(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Path(template_id): Path<Uuid>,
    Json(body): Json<SetVisibilityRequest>,
) -> Result<StatusCode, ApiError> {
    let tenant_id = ctx.tenant_id;
    marketplace::set_visibility(
        &state.identity_pool,
        tenant_id,
        template_id,
        &body.visibility,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
