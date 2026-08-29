//! Following an origin template: `/api/schemas/upstream-changes`, `merge-preview` and `merge`.
//! This overlays base's own `/api/schemas` namespace, since merging a schema is a schema operation from the client's side, not an administrative one like the dashboard or Stripe.
//!
//! Authentication goes through [`authz::authenticate_workspace`] rather than a base extractor. That was once forced by the crate split; it is now a choice this module keeps, because the extractors carry their scope requirement in the handler's signature and these handlers state theirs explicitly below.
//! It resolves through `TenantScopedAuthenticator`, so a workspace-scoped key names its own workspace and a tenant-scoped one names it with `X-Workspace-Id`.
//! The scope check is explicit for the same reason: there is no extractor here to carry it.

use crate::controllers::ApiError;
use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::VersioningDiff;
use crate::models::content_schemas::{SchemaRecord, UpstreamChange};
use crate::services::auth::{ApiKeyScope, require_scope};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::Serialize;
use uuid::Uuid;

use crate::ee::models::origin as origin_model;
use crate::ee::services::merge::MergePlan;
use crate::ee::services::{authz, origin};

/// Base's own extractors enforce a minimum scope by type; without them here, the check is written out explicitly.
/// The response of a merge, matching the community edition's schema-creation response shape so a client written against that response shape needs no change.
#[derive(Debug, Serialize)]
pub struct MergeResponse {
    pub schema: SchemaRecord,
    pub diff: VersioningDiff,
}

/// `GET /api/schemas/upstream-changes`: schemas whose origin template has moved on.
async fn list_upstream_changes(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(page): Query<crate::controllers::PageParams>,
) -> Result<Json<Vec<UpstreamChange>>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;

    // ctx.db: this joins identity_templates, which the request role cannot read.
    let changes =
        origin_model::list_with_upstream_changes(&ctx.db, auth_ctx.workspace_id, page.into())
            .await?;
    Ok(Json(changes))
}

/// `GET /api/schemas/{schema_id}/merge-preview`: what following the template would do.
async fn merge_preview(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(schema_id): Path<Uuid>,
) -> Result<Json<MergePlan>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;

    let db = ctx
        .shared_store
        .get::<crate::db::DbHandle>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
    // The schema is workspace content and comes off the RLS-scoped connection; the template is control-plane data the request role holds no grant on, hence both `schema_txn` and `ctx`.
    let schema_txn = db
        .tenant
        .begin_for_workspace(auth_ctx.tenant_id, auth_ctx.workspace_id)
        .await
        .internal()?;
    let plan = origin::merge_preview(
        &schema_txn,
        &ctx,
        auth_ctx.tenant_id,
        auth_ctx.workspace_id,
        schema_id,
    )
    .await?;
    // Read-only: rolling back (rather than committing) a transaction that made no writes is equivalent, and dropping it does exactly that.
    Ok(Json(plan))
}

/// `POST /api/schemas/{schema_id}/merge`: write the merged definition as the next version.
async fn merge_apply(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(schema_id): Path<Uuid>,
) -> Result<(StatusCode, Json<MergeResponse>), ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;

    let db = ctx
        .shared_store
        .get::<crate::db::DbHandle>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
    let schema_txn = db
        .tenant
        .begin_for_workspace(auth_ctx.tenant_id, auth_ctx.workspace_id)
        .await
        .internal()?;
    let (schema, diff) = origin::merge_apply(
        &schema_txn,
        &ctx,
        auth_ctx.tenant_id,
        auth_ctx.workspace_id,
        schema_id,
    )
    .await?;
    schema_txn.commit().await.internal()?;
    Ok((StatusCode::CREATED, Json(MergeResponse { schema, diff })))
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
