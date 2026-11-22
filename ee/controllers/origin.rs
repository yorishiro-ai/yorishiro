//! Following an origin template: `/api/schemas/upstream-changes`, `merge-preview` and `merge`.
//!
//! This overlays base's own `/api/schemas` namespace, since merging a schema is a schema operation from the client's side, not an administrative one like the dashboard or Stripe.
//!
use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, SchemaScope, Verified};
use crate::controllers::openapi::Routes;
use crate::models::schema_schemas::UpstreamChange;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::edition::ee::dtos::origin::MergeResponse;
use crate::edition::ee::models::schema_schemas as origin_model;
use crate::edition::ee::models::schema_schemas as origin;
use crate::edition::ee::models::schema_schemas::MergePlan;

/// `GET /api/schemas/upstream-changes`: schemas whose origin template has moved on.
async fn list_upstream_changes(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<UpstreamChange>>, ApiError> {
    let changes = origin_model::list_for_actor(&ctx, &verified.ctx, page.try_into()?).await?;
    Ok(Json(changes))
}

/// `GET /api/schemas/{schema_id}/merge-preview`: what following the template would do.
async fn merge_preview(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(crate::dtos::common::SchemaIdPath { schema_id }): Path<
        crate::dtos::common::SchemaIdPath<Uuid>,
    >,
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
async fn merge_apply(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(crate::dtos::common::SchemaIdPath { schema_id }): Path<
        crate::dtos::common::SchemaIdPath<Uuid>,
    >,
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

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/schemas")
        .add(
            "/upstream-changes",
            aide::axum::routing::get_with(list_upstream_changes, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["schema_upstream_list"]),
                    ))
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{schema_id}/merge-preview",
            aide::axum::routing::get_with(merge_preview, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["schema_merge_preview"]),
                    ))
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{schema_id}/merge",
            aide::axum::routing::post_with(merge_apply, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["schema"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["schema_merge_apply"]),
                    ))
                    .response::<201, Json<MergeResponse>>()
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
}
