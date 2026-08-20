//! Following an origin template: `/api/schemas/upstream-changes`, `merge-preview` and `merge`.
//!
//! Creating a schema from a template is base's responsibility, untouched by this crate; flowing a template's later edits into the copies is this edition's.
//!
//! Two things differ from the community version, both forced rather than chosen:
//!
//! * Authentication goes through [`authz::authenticate_workspace`] rather than an `Authorized` extractor, since this crate's lib may not depend on `yorishiro-server`.
//!   That helper resolves through `TenantScopedAuthenticator`, so a workspace-scoped key names its own workspace and a tenant-scoped one names it with `X-Workspace-Id`.
//!   Both work here, where the community version only ever saw the first kind.
//! * The scope check is explicit: the handler checks the scope itself, because there is no extractor here to carry it.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;
use yorishiro_core::ResultExt;
use yorishiro_core::error::YorishiroError;
use yorishiro_core::metaschema::VersioningDiff;
use yorishiro_core::models::schemas::{SchemaRecord, UpstreamChange};
use yorishiro_core::services::auth::{ApiKeyScope, AuthContext};

use crate::error::HostedApiError;
use crate::models::origin as origin_model;
use crate::services::merge::MergePlan;
use crate::services::{authz, origin};
use crate::state::HostedState;

/// The community edition's extractors enforce a minimum scope by type.
/// Without them, the check is written out: the ordering on `ApiKeyScope` is the same one they use.
fn require_scope(ctx: &AuthContext, needed: ApiKeyScope) -> Result<(), YorishiroError> {
    if ctx.scope < needed {
        return Err(YorishiroError::ScopeInsufficient {
            message: format!("this endpoint needs the {needed:?} scope or higher"),
            hint: "issue a key with a higher scope".into(),
        });
    }
    Ok(())
}

/// The response of a merge, matching the community edition's `CreateSchemaResponse` shape so a client written against that response shape needs no change.
#[derive(Debug, Serialize, ToSchema)]
pub struct MergeResponse {
    pub schema: SchemaRecord,
    pub diff: VersioningDiff,
}

/// `GET /api/schemas/upstream-changes`: schemas whose origin template has moved on.
#[utoipa::path(
    get,
    path = "/api/schemas/upstream-changes",
    responses(
        (status = 200, description = "Schemas whose origin template has changed since the copy was taken", body = Vec<UpstreamChange>),
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::HostedApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::HostedApiErrorBody),
    ),
    security(("bearer_key" = [])),
    tag = "origin",
)]
pub async fn list_upstream_changes(
    State(state): State<HostedState>,
    headers: HeaderMap,
) -> Result<Json<Vec<UpstreamChange>>, HostedApiError> {
    let ctx = authz::authenticate_workspace(&state, &headers).await?;
    require_scope(&ctx, ApiKeyScope::Read)?;

    // The control-plane pool: this joins identity.templates, which the request role cannot read.
    let changes =
        origin_model::list_with_upstream_changes(&state.identity_pool, ctx.workspace_id).await?;
    Ok(Json(changes))
}

/// `GET /api/schemas/{schema_id}/merge-preview`: what following the template would do.
#[utoipa::path(
    get,
    path = "/api/schemas/{schema_id}/merge-preview",
    params(("schema_id" = Uuid, Path, description = "Schema ID")),
    responses(
        (status = 200, description = "What following the origin template would do", body = MergePlan),
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::HostedApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::HostedApiErrorBody),
        (status = 404, description = "The schema does not exist", body = crate::error::HostedApiErrorBody),
        (status = 422, description = "The schema follows no template, or has no recorded merge base", body = crate::error::HostedApiErrorBody),
    ),
    security(("bearer_key" = [])),
    tag = "origin",
)]
pub async fn merge_preview(
    State(state): State<HostedState>,
    headers: HeaderMap,
    Path(schema_id): Path<Uuid>,
) -> Result<Json<MergePlan>, HostedApiError> {
    let ctx = authz::authenticate_workspace(&state, &headers).await?;
    require_scope(&ctx, ApiKeyScope::Read)?;

    // The schema is workspace content and comes off the RLS-scoped connection; the template is control-plane data the request role holds no grant on, hence both.
    let mut conn = state
        .tenant_db
        .acquire_for_workspace(ctx.tenant_id, ctx.workspace_id)
        .await
        .internal()?;
    let plan = origin::merge_preview(
        &mut conn,
        &state.identity_pool,
        ctx.tenant_id,
        ctx.workspace_id,
        schema_id,
    )
    .await?;
    Ok(Json(plan))
}

/// `POST /api/schemas/{schema_id}/merge`: write the merged definition as the next version.
#[utoipa::path(
    post,
    path = "/api/schemas/{schema_id}/merge",
    params(("schema_id" = Uuid, Path, description = "Schema ID")),
    responses(
        (status = 201, description = "Merged definition written as the schema's next version", body = MergeResponse),
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::HostedApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::HostedApiErrorBody),
        (status = 404, description = "The schema does not exist", body = crate::error::HostedApiErrorBody),
        (status = 409, description = "Version conflict due to concurrent creation", body = crate::error::HostedApiErrorBody),
        (status = 422, description = "The schema follows no template, has no recorded merge base, or the merge has conflicts", body = crate::error::HostedApiErrorBody),
    ),
    security(("bearer_key" = [])),
    tag = "origin",
)]
pub async fn merge_apply(
    State(state): State<HostedState>,
    headers: HeaderMap,
    Path(schema_id): Path<Uuid>,
) -> Result<(StatusCode, Json<MergeResponse>), HostedApiError> {
    let ctx = authz::authenticate_workspace(&state, &headers).await?;
    require_scope(&ctx, ApiKeyScope::Schema)?;

    let mut conn = state
        .tenant_db
        .acquire_for_workspace(ctx.tenant_id, ctx.workspace_id)
        .await
        .internal()?;
    let (schema, diff) = origin::merge_apply(
        &mut conn,
        &state.identity_pool,
        ctx.tenant_id,
        ctx.workspace_id,
        schema_id,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(MergeResponse { schema, diff })))
}
