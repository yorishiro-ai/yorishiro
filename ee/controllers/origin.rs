//! Following an origin template: `/api/schemas/upstream-changes`, `merge-preview` and `merge`.
//! This overlays base's own `/api/schemas` namespace, since merging a schema is a schema operation from the client's side, not an administrative one like the dashboard or Stripe.
//!
use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, SchemaScope};
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
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/schemas/upstream-changes", params(("page" = Option<i32>, Query), ("page_size" = Option<i32>, Query)), responses((status = 200, body = [crate::edition::ee::controllers::openapi::UpstreamChange]), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["schema_upstream_list"]))), tag = "enterprise"))]
async fn list_upstream_changes(
    State(ctx): State<AppContext>,
    mut authorized: Authorized<ReadScope>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<UpstreamChange>>, ApiError> {
    authorized
        .require_action(crate::models::iam_policy::Action::SchemaUpstreamList)
        .await?;

    // Upstream joins templates through the identity pool under the retained IAM fence.
    let changes = origin_model::list_with_upstream_changes(
        &ctx.db,
        authorized.ctx.require_workspace()?,
        page.try_into()?,
    )
    .await?;
    Ok(Json(changes))
}

/// `GET /api/schemas/{schema_id}/merge-preview`: what following the template would do.
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/schemas/{schema_id}/merge-preview", params(("schema_id" = Uuid, Path)), responses((status = 200, body = crate::edition::ee::controllers::openapi::MergePlan), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["schema_merge_preview"]))), tag = "enterprise"))]
async fn merge_preview(
    State(ctx): State<AppContext>,
    mut authorized: Authorized<ReadScope>,
    Path(schema_id): Path<Uuid>,
) -> Result<Json<MergePlan>, ApiError> {
    authorized
        .require_action(crate::models::iam_policy::Action::SchemaMergePreview)
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
#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/schemas/{schema_id}/merge", params(("schema_id" = Uuid, Path)), responses((status = 201, body = crate::edition::ee::controllers::openapi::MergeResponse), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"])), ("x-yorishiro-required-actions" = json!(["schema_merge_apply"]))), tag = "enterprise"))]
async fn merge_apply(
    State(ctx): State<AppContext>,
    mut authorized: Authorized<SchemaScope>,
    Path(schema_id): Path<Uuid>,
) -> Result<(StatusCode, Json<MergeResponse>), ApiError> {
    authorized
        .require_action(crate::models::iam_policy::Action::SchemaMergeApply)
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
pub(crate) fn openapi_docs() -> Vec<crate::controllers::route_inventory::RouteDoc> {
    vec![
        crate::controllers::route_inventory::path_doc(__path_list_upstream_changes),
        crate::controllers::route_inventory::path_doc(__path_merge_preview),
        crate::controllers::route_inventory::path_doc(__path_merge_apply),
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
