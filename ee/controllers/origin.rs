//! Following an origin template: `/api/schemas/upstream-changes`, `merge-preview` and `merge`.
//!
//! These routes share the `/api/schemas` prefix because merging is a schema operation from the client's side.

use crate::controllers::ApiError;
use crate::controllers::Created;
use crate::controllers::extractors::Verified;
use crate::controllers::openapi::Routes;
use crate::models::schema_schemas::UpstreamChange;
use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::edition::ee::dtos::origin::MergeResponse;
use crate::edition::ee::models::schema_schemas as origin;
use crate::edition::ee::models::schema_schemas::MergePlan;

/// `GET /api/schemas/upstream-changes`: schemas whose origin template has moved on.
async fn list_upstream_changes(
    State(ctx): State<AppContext>,
    verified: Verified,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<UpstreamChange>>, ApiError> {
    let changes = origin::list_for_actor(&ctx, &verified.ctx, page.try_into()?).await?;
    Ok(Json(changes))
}

/// `GET /api/schemas/{schema_id}/merge-preview`: what following the template would do.
async fn merge_preview(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::SchemaIdPath { schema_id }): Path<
        crate::dtos::common::SchemaIdPath<Uuid>,
    >,
) -> Result<Json<MergePlan>, ApiError> {
    Ok(Json(
        origin::merge_preview_for_actor(&ctx, verified.ctx, schema_id).await?,
    ))
}

/// `POST /api/schemas/{schema_id}/merge`: write the merged definition as the next version.
async fn merge_apply(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::SchemaIdPath { schema_id }): Path<
        crate::dtos::common::SchemaIdPath<Uuid>,
    >,
) -> Result<Created<MergeResponse>, ApiError> {
    Ok(Created(
        origin::merge_apply_for_actor(&ctx, verified.ctx, schema_id).await?,
    ))
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("enterprise")
        .prefix("api/schemas")
        .add(
            "/upstream-changes",
            aide::axum::routing::get_with(list_upstream_changes, |op| {
                op.id("ListSchemaUpstreamChanges")
            }),
        )
        .add(
            "/{schema_id}/merge-preview",
            aide::axum::routing::get_with(merge_preview, |op| op.id("PreviewSchemaMerge")),
        )
        .add(
            "/{schema_id}/merge",
            aide::axum::routing::post_with(merge_apply, |op| op.id("ApplySchemaMerge")),
        )
}
