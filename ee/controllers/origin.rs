//! Following an origin template: `/api/schemas/upstream-changes`, `merge-preview` and `merge`.
//!
//! This overlays base's own `/api/schemas` namespace, since merging a schema is a schema operation from the client's side, not an administrative one like the dashboard or Stripe.
//!
use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, SchemaScope, Verified};
use crate::models::schema_schemas::UpstreamChange;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use uuid::Uuid;

use crate::edition::ee::dtos::origin::MergeResponse;
use crate::edition::ee::models::schema_schemas as origin_model;
use crate::edition::ee::models::schema_schemas as origin;
use crate::edition::ee::models::schema_schemas::MergePlan;

/// `GET /api/schemas/upstream-changes`: schemas whose origin template has moved on.
#[cfg(feature = "openapi")]
fn list_upstream_changes_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schemas/upstream-changes",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_upstream_list"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<UpstreamChange>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "page", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}, {"name": "page_size", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}]}),
    )
}
async fn list_upstream_changes(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<UpstreamChange>>, ApiError> {
    let changes = origin_model::list_for_actor(&ctx, &verified.ctx, page.try_into()?).await?;
    Ok(Json(changes))
}

/// `GET /api/schemas/{schema_id}/merge-preview`: what following the template would do.
#[cfg(feature = "openapi")]
fn merge_preview_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schemas/{schema_id}/merge-preview",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_merge_preview"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<MergePlan>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "schema_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
async fn merge_preview(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(schema_id): Path<Uuid>,
) -> Result<Json<MergePlan>, ApiError> {
    let mut authorized = Authorized::<ReadScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Schema(schema_id),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaMergePreview)
        .await?;

    // Workspace content uses the scoped transaction; templates require the identity pool.
    let plan = origin::merge_preview(
        authorized.txn(),
        &ctx,
        authorized.ctx.tenant_id,
        authorized.ctx.require_workspace()?,
        schema_id,
    )
    .await?;
    // Read-only: rolling back (rather than committing) a transaction that made no writes is equivalent, and dropping it does exactly that.
    Ok(Json(plan))
}

/// `POST /api/schemas/{schema_id}/merge`: write the merged definition as the next version.
#[cfg(feature = "openapi")]
fn merge_apply_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schemas/{schema_id}/merge",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_merge_apply"]), "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<MergeResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "schema_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
async fn merge_apply(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(schema_id): Path<Uuid>,
) -> Result<(StatusCode, Json<MergeResponse>), ApiError> {
    let mut authorized = Authorized::<SchemaScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Schema(schema_id),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaMergeApply)
        .await?;

    let (schema, diff, summary) = origin::merge_apply(
        authorized.txn(),
        &ctx,
        authorized.ctx.tenant_id,
        authorized.ctx.require_workspace()?,
        schema_id,
    )
    .await?;
    authorized.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(MergeResponse {
            schema,
            diff,
            summary,
        }),
    ))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::openapi::route_inventory::RouteDoc> {
    vec![
        list_upstream_changes_doc(),
        merge_preview_doc(),
        merge_apply_doc(),
    ]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/schemas")
        .add(
            "/upstream-changes",
            axum::routing::get(list_upstream_changes),
        )
        .add(
            "/{schema_id}/merge-preview",
            axum::routing::get(merge_preview),
        )
        .add("/{schema_id}/merge", axum::routing::post(merge_apply))
}
