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
#[cfg(feature = "openapi")]
fn list_marketplace_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/marketplace",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["marketplace_list"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<MarketplaceListing>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "page", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}, {"name": "page_size", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}]}),
    )
}
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
#[cfg(feature = "openapi")]
fn list_versions_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/marketplace/{id}/versions",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["marketplace_version_list"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<TemplateVersionRecord>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}, {"name": "page", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}, {"name": "page_size", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}]}),
    )
}
async fn list_versions(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<TemplateVersionRecord>>, ApiError> {
    let actor = authz::authenticate(&ctx, &headers).await?;
    let versions =
        marketplace_models::versions_as(&ctx.db, &actor, template_id, page.try_into()?).await?;
    Ok(Json(versions))
}

/// `POST /api/marketplace/{id}/versions`: publish the next version of your own template.
#[cfg(feature = "openapi")]
fn publish_version_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/marketplace/{id}/versions",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["enterprise"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::edition::ee::controllers::openapi::PublishVersionRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["marketplace_publish"]), "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<TemplateVersionRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
async fn publish_version(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
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
#[cfg(feature = "openapi")]
fn list_reviews_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/marketplace/{id}/reviews",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["marketplace_review_list"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<TemplateReviewRecord>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}, {"name": "page", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}, {"name": "page_size", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}]}),
    )
}
async fn list_reviews(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<TemplateReviewRecord>>, ApiError> {
    let actor = authz::authenticate(&ctx, &headers).await?;
    let reviews =
        marketplace_models::reviews_as(&ctx.db, &actor, template_id, page.try_into()?).await?;
    Ok(Json(reviews))
}

/// `POST /api/marketplace/{id}/reviews`: leave or replace this tenant's review.
#[cfg(feature = "openapi")]
fn submit_review_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/marketplace/{id}/reviews",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["enterprise"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<SubmitReviewRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["marketplace_review_submit"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<TemplateReviewRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
async fn submit_review(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Json(body): Json<SubmitReviewRequest>,
) -> Result<Json<TemplateReviewRecord>, ApiError> {
    let actor = authz::authenticate(&ctx, &headers).await?;
    let record = marketplace::submit_review(&ctx, &actor, template_id, body).await?;
    Ok(Json(record))
}

/// `POST /api/marketplace/{id}/fork`: copy a published version into your own library.
#[cfg(feature = "openapi")]
fn fork_template_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/marketplace/{id}/fork",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["marketplace_fork"]), "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<ForkResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}, {"name": "version", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}]}),
    )
}
async fn fork_template(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
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
#[cfg(feature = "openapi")]
fn set_visibility_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/marketplace/{id}/visibility",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["enterprise"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<SetVisibilityRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["marketplace_visibility_update"]), "responses": {"204": serde_json::json!({"description": "Marketplace visibility updated"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
async fn set_visibility(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Json(body): Json<SetVisibilityRequest>,
) -> Result<StatusCode, ApiError> {
    let actor = authz::authenticate(&ctx, &headers).await?;
    let visibility = TemplateVisibility::parse_input(&body.visibility)?;
    marketplace::set_visibility(&ctx, &actor, template_id, visibility).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::openapi::route_inventory::RouteDoc> {
    vec![
        list_marketplace_doc(),
        list_versions_doc(),
        publish_version_doc(),
        list_reviews_doc(),
        submit_review_doc(),
        fork_template_doc(),
        set_visibility_doc(),
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
