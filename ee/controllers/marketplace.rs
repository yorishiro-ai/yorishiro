//! The template marketplace: templates shared between tenants, their published versions, and what other tenants thought of them.
//!
//! These routes gate on the licence: no key means `GET /api/marketplace` and its siblings answer 404.
//! Not every route under `ee/` is gated, so this does not follow from living here or from sharing the `api/` prefix with the community edition.
//! Which routes carry the gate is decided in `app.rs`, and that is the list to read.

use crate::controllers::ApiError;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use uuid::Uuid;

use crate::ee::controllers::middleware::auth as authz;
use crate::ee::dtos::marketplace::{
    ForkParams, ForkResponse, PublishVersionRequest as PublishVersionInput, SetVisibilityRequest,
};
use crate::ee::models::marketplace;
use crate::ee::models::marketplace::{
    self as marketplace_models, MarketplaceListing, PublishVersionRequest, SubmitReviewRequest,
    TemplateReviewRecord, TemplateVersionRecord, TemplateVersionStatus,
};
use crate::models::template_templates::TemplateVisibility;

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
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/marketplace", params(("page" = Option<i32>, Query), ("page_size" = Option<i32>, Query)), responses((status = 200, body = [crate::ee::controllers::openapi::MarketplaceListing]), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), tag = "enterprise"))]
async fn list_marketplace(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<MarketplaceListing>>, ApiError> {
    // The listing spans every tenant, so the identity is not read, but a valid key is still required, which is what authenticating here enforces.
    let _ = licensed_tenant(&ctx, &headers).await?;
    let listings = marketplace_models::list_marketplace(&ctx.db, page.into()).await?;
    Ok(Json(listings))
}

/// `GET /api/marketplace/{id}/versions`: published versions, plus the caller's own drafts when it owns the template.
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/marketplace/{id}/versions", params(("id" = Uuid, Path), ("page" = Option<i32>, Query), ("page_size" = Option<i32>, Query)), responses((status = 200, body = [crate::ee::controllers::openapi::TemplateVersionRecord]), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), tag = "enterprise"))]
async fn list_versions(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<TemplateVersionRecord>>, ApiError> {
    let (tenant_id, _) = licensed_tenant(&ctx, &headers).await?;
    let versions =
        marketplace_models::list_versions(&ctx.db, tenant_id, template_id, page.into()).await?;
    Ok(Json(versions))
}

/// `POST /api/marketplace/{id}/versions`: publish the next version of your own template.
#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/marketplace/{id}/versions", params(("id" = Uuid, Path)), request_body = crate::ee::controllers::openapi::PublishVersionRequest, responses((status = 201, body = crate::ee::controllers::openapi::TemplateVersionRecord), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody), (status = 422, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), tag = "enterprise"))]
async fn publish_version(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Json(body): Json<PublishVersionInput>,
) -> Result<(StatusCode, Json<TemplateVersionRecord>), ApiError> {
    let (tenant_id, user_id) = licensed_tenant(&ctx, &headers).await?;
    let request = PublishVersionRequest {
        definition: body.definition,
        changelog: body.changelog,
        status: TemplateVersionStatus::parse_publish(&body.status)?,
    };
    let record =
        marketplace::publish_version(&ctx, tenant_id, template_id, user_id, request).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

/// `GET /api/marketplace/{id}/reviews`
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/marketplace/{id}/reviews", params(("id" = Uuid, Path), ("page" = Option<i32>, Query), ("page_size" = Option<i32>, Query)), responses((status = 200, body = [crate::ee::controllers::openapi::TemplateReviewRecord]), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), tag = "enterprise"))]
async fn list_reviews(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<TemplateReviewRecord>>, ApiError> {
    let (tenant_id, _) = licensed_tenant(&ctx, &headers).await?;
    let reviews =
        marketplace_models::list_reviews(&ctx.db, tenant_id, template_id, page.into()).await?;
    Ok(Json(reviews))
}

/// `POST /api/marketplace/{id}/reviews`: leave or replace this tenant's review.
#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/marketplace/{id}/reviews", params(("id" = Uuid, Path)), request_body = crate::ee::controllers::openapi::SubmitReviewRequest, responses((status = 200, body = crate::ee::controllers::openapi::TemplateReviewRecord), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody), (status = 422, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), tag = "enterprise"))]
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

/// `POST /api/marketplace/{id}/fork`: copy a published version into your own library.
#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/marketplace/{id}/fork", params(("id" = Uuid, Path), ("version" = Option<i32>, Query)), responses((status = 201, body = crate::ee::controllers::openapi::ForkResponse), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), tag = "enterprise"))]
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

/// `PUT /api/marketplace/{id}/visibility`: list your own template, or take it back down.
#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/marketplace/{id}/visibility", params(("id" = Uuid, Path)), request_body = crate::ee::controllers::openapi::SetVisibilityRequest, responses((status = 204, description = "Marketplace visibility updated"), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody), (status = 422, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), tag = "enterprise"))]
async fn set_visibility(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Json(body): Json<SetVisibilityRequest>,
) -> Result<StatusCode, ApiError> {
    let (tenant_id, _) = licensed_tenant(&ctx, &headers).await?;
    let visibility = TemplateVisibility::parse_input(&body.visibility)?;
    marketplace::set_visibility(&ctx, tenant_id, template_id, visibility).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::route_inventory::RouteDoc> {
    vec![
        crate::controllers::route_inventory::path_doc(__path_list_marketplace),
        crate::controllers::route_inventory::path_doc(__path_list_versions),
        crate::controllers::route_inventory::path_doc(__path_publish_version),
        crate::controllers::route_inventory::path_doc(__path_list_reviews),
        crate::controllers::route_inventory::path_doc(__path_submit_review),
        crate::controllers::route_inventory::path_doc(__path_fork_template),
        crate::controllers::route_inventory::path_doc(__path_set_visibility),
    ]
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
