//! Inferring values for fields an entity is missing, and the per-workspace credentials it runs on.
//!
//! This product does not pay for inference, so a workspace brings its own key.
//! A workspace with none configured gets a 422 rather than a fall back to `default` values: a caller who asked for inference and silently received defaults would have no way to tell that nothing was inferred.

use crate::controllers::ApiError;
use crate::controllers::middleware::auth::require_scope;
use crate::db::AppContextBackend;
use crate::error::{ResultExt, YorishiroError};
use crate::models::api_keys::{ApiKeyScope, AuthContext};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::TransactionTrait;

use crate::ee::controllers::middleware::auth as authz;
use crate::ee::dtos::inference::{InferFillResponse, InferJobStatusResponse, SetLlmKeyRequest};
use crate::ee::models::inference_jobs;
use crate::ee::models::inference_jobs::InferenceJobStatus;
use crate::ee::models::inference_proposals;
use crate::ee::models::workspace_llm_keys;

/// `POST /api/schemas/active/{name}/infer-fill`
///
/// Enqueues an async infer-fill job whose answers are stored as reviewable proposals.
///
/// Poll for completion via `GET /api/inference-jobs/{job_id}`.
#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/schemas/active/{name}/infer-fill", params(("name" = String, Path)), responses((status = 200, body = crate::ee::controllers::openapi::InferFillResponse), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody), (status = 422, body = crate::controllers::openapi::ApiErrorBody), (status = 503, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "enterprise"))]
async fn infer_fill(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<InferFillResponse>, ApiError> {
    // The server calls an LLM here, which is the definition of an enterprise feature. The licence check is
    // `app::licence_gate`, a layer on this route's own group: it runs before authentication, so an
    // unlicensed deployment answers the same 404 to everyone rather than confirming to a valid key
    // that the endpoint exists and is merely locked.
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    let workspace_id = auth_ctx.workspace_id;

    // Refuse before doing any work: a caller with no key gets one clear error rather than a scan that reports zero applied and reads as "nothing to infer".
    if workspace_llm_keys::get(&ctx.db, workspace_id)
        .await
        .internal()?
        .is_none()
    {
        return Err(YorishiroError::ValidationFailed {
            message: "this workspace has no LLM credentials configured".into(),
            details: vec![],
            hint: "PUT /api/workspace/llm-key".to_string(),
        }
        .into());
    }

    let job_id = crate::ee::workers::infer_fill::enqueue_infer_fill(&ctx, workspace_id, name)
        .await
        .internal()?;

    Ok(Json(InferFillResponse {
        job_id,
        status: InferenceJobStatus::Queued,
    }))
}

/// `GET /api/inference-jobs/{job_id}`
///
/// Poll for the durable result of an infer-fill job.
///
/// Requires authentication and read scope, and only returns a job belonging to the
/// authenticated workspace.
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/inference-jobs/{job_id}", params(("job_id" = String, Path)), responses((status = 200, body = crate::ee::controllers::openapi::InferJobStatusResponse), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"]))), tag = "enterprise"))]
async fn infer_job_status(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<InferJobStatusResponse>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let caller_workspace_id = auth_ctx.workspace_id;
    let parsed_job_id = uuid::Uuid::parse_str(&job_id)
        .map_err(|_| YorishiroError::not_found("infer-fill job not found"))?;
    let result = inference_jobs::get(&ctx.db, parsed_job_id)
        .await?
        .ok_or_else(|| YorishiroError::not_found("infer-fill job not found"))?;

    // Enforce workspace isolation: the caller can only poll jobs belonging to their own workspace.
    if result.workspace_id != caller_workspace_id {
        return Err(YorishiroError::ScopeInsufficient {
            message: "cannot access infer-fill jobs for another workspace".into(),
            hint: "verify the job belongs to your workspace".into(),
        }
        .into());
    }

    let status = result.status;
    let completed = status == InferenceJobStatus::Completed;
    Ok(Json(InferJobStatusResponse {
        job_id,
        status,
        applied: completed.then_some(result.applied),
        proposed: completed.then_some(result.proposed),
        skipped: completed.then_some(result.skipped),
        error: result.error,
    }))
}

/// Managing the workspace's own LLM credentials.
///
/// Separate from [`gated_routes`] because the licence gate applies per `Routes` and these three are
/// not gated: storing a credential does not require a licence, only spending it on an inference call
/// does. Merging the two groups would extend the gate over these, which is a product decision rather
/// than a routing detail.
///
/// The two groups also sit under different prefixes: credentials belong with the workspace's other
/// settings, while the inference call itself extends the schema routes it fills against.
pub fn routes() -> Routes {
    Routes::new().prefix("api/workspace").add(
        "/llm-key",
        axum::routing::put(set_llm_key)
            .get(get_llm_key)
            .delete(delete_llm_key),
    )
}

/// The route this module's licence gate covers: the server calls an LLM here, which is what makes
/// it an enterprise feature. See [`routes`] for why the two groups are separate.
pub fn gated_routes() -> Routes {
    Routes::new()
        .prefix("api/schemas")
        .add("/active/{name}/infer-fill", axum::routing::post(infer_fill))
}

/// `GET /api/inference-jobs/{job_id}`
///
/// Poll for the result of an infer-fill job. Follows the same pattern as
/// `migration_routes()` (`api/migration-jobs/{job_id}/undo`): a dedicated prefix for
/// async-job polling, under the licence gate because results include workspace
/// activity counts that reveal whether a workspace has data to infer.
pub fn inference_job_status_routes() -> Routes {
    Routes::new()
        .prefix("api/inference-jobs")
        .add("/{job_id}", axum::routing::get(infer_job_status))
        .add("/{job_id}/proposals", axum::routing::get(list_proposals))
        .add("/{job_id}/confirm", axum::routing::post(confirm_proposals))
        .add("/{job_id}/reject", axum::routing::post(reject_proposals))
        .add("/{job_id}/discard", axum::routing::post(discard_proposals))
}

async fn authorized_job(
    ctx: &AppContext,
    headers: &HeaderMap,
    job_id: &str,
    scope: ApiKeyScope,
) -> Result<(AuthContext, uuid::Uuid), ApiError> {
    let auth_ctx = authz::authenticate_workspace(ctx, headers).await?;
    require_scope(&auth_ctx, scope)?;
    let parsed = uuid::Uuid::parse_str(job_id)
        .map_err(|_| YorishiroError::not_found("infer-fill job not found"))?;
    let job = inference_jobs::get(&ctx.db, parsed)
        .await?
        .ok_or_else(|| YorishiroError::not_found("infer-fill job not found"))?;
    if job.workspace_id != auth_ctx.workspace_id {
        return Err(YorishiroError::ScopeInsufficient {
            message: "cannot access infer-fill jobs for another workspace".into(),
            hint: "verify the job belongs to your workspace".into(),
        }
        .into());
    }
    Ok((auth_ctx, parsed))
}

/// `GET /api/inference-jobs/{job_id}/proposals`
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/inference-jobs/{job_id}/proposals", params(("job_id" = String, Path)), responses((status = 200, body = [crate::ee::controllers::openapi::InferenceProposalResponse]), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"]))), tag = "enterprise"))]
async fn list_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<Vec<inference_proposals::ProposalRecord>>, ApiError> {
    let (auth_ctx, job_id) = authorized_job(&ctx, &headers, &job_id, ApiKeyScope::Read).await?;
    let txn = proposal_transaction(&ctx, &auth_ctx).await?;
    let proposals = inference_proposals::for_job(&txn, auth_ctx.workspace_id, job_id).await?;
    drop(txn);
    Ok(Json(proposals))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/inference-jobs/{job_id}/reject", params(("job_id" = String, Path)), responses((status = 200, body = crate::ee::controllers::openapi::ProposalActionResponse), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "enterprise"))]
async fn reject_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<inference_proposals::ProposalActionReport>, ApiError> {
    let (auth_ctx, job_id) = authorized_job(&ctx, &headers, &job_id, ApiKeyScope::Schema).await?;
    let txn = proposal_transaction(&ctx, &auth_ctx).await?;
    let report = inference_proposals::reject(&txn, auth_ctx.workspace_id, job_id).await?;
    txn.commit().await.internal()?;
    Ok(Json(report))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/inference-jobs/{job_id}/discard", params(("job_id" = String, Path)), responses((status = 200, body = crate::ee::controllers::openapi::ProposalActionResponse), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "enterprise"))]
async fn discard_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<inference_proposals::ProposalActionReport>, ApiError> {
    let (auth_ctx, job_id) = authorized_job(&ctx, &headers, &job_id, ApiKeyScope::Schema).await?;
    let txn = proposal_transaction(&ctx, &auth_ctx).await?;
    let report = inference_proposals::discard(&txn, auth_ctx.workspace_id, job_id).await?;
    txn.commit().await.internal()?;
    Ok(Json(report))
}

async fn proposal_transaction(
    ctx: &AppContext,
    auth_ctx: &AuthContext,
) -> Result<sea_orm::DatabaseTransaction, ApiError> {
    if ctx.is_sqlite() {
        return Ok(ctx.db.begin().await.internal()?);
    }
    let db = ctx
        .shared_store
        .get::<crate::db::DbHandle>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
    Ok(db
        .tenant
        .begin_for_workspace(auth_ctx.tenant_id, auth_ctx.workspace_id)
        .await
        .internal()?)
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/inference-jobs/{job_id}/confirm", params(("job_id" = String, Path)), responses((status = 200, body = crate::ee::controllers::openapi::ProposalConfirmResponse), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody), (status = 409, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "enterprise"))]
async fn confirm_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<inference_proposals::ConfirmReport>, ApiError> {
    let (auth_ctx, job_id) = authorized_job(&ctx, &headers, &job_id, ApiKeyScope::Schema).await?;
    let txn = proposal_transaction(&ctx, &auth_ctx).await?;
    let report =
        inference_proposals::confirm(&txn, auth_ctx.workspace_id, job_id, auth_ctx.user_id).await?;
    txn.commit().await.internal()?;
    Ok(Json(report))
}

/// `PUT /api/workspace/llm-key`
#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/workspace/llm-key", request_body = crate::ee::controllers::openapi::LlmKeyRequest, responses((status = 204, description = "LLM credentials saved"), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 422, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "enterprise"))]
async fn set_llm_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(body): Json<SetLlmKeyRequest>,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    workspace_llm_keys::set(
        &ctx.db,
        auth_ctx.workspace_id,
        &body.base_url,
        &body.model,
        &body.api_key,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/workspace/llm-key`
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/workspace/llm-key", responses((status = 200, body = crate::ee::controllers::openapi::LlmKeyResponse), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"]))), tag = "enterprise"))]
async fn get_llm_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<workspace_llm_keys::LlmKeyDescription>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let described = workspace_llm_keys::describe(&ctx.db, auth_ctx.workspace_id)
        .await?
        .ok_or_else(|| YorishiroError::not_found("no LLM credentials configured"))?;
    Ok(Json(described))
}

/// `DELETE /api/workspace/llm-key`
#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/workspace/llm-key", responses((status = 204, description = "LLM credentials removed"), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "enterprise"))]
async fn delete_llm_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    workspace_llm_keys::clear(&ctx.db, auth_ctx.workspace_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::route_inventory::RouteDoc> {
    vec![
        crate::controllers::route_inventory::path_doc(__path_set_llm_key),
        crate::controllers::route_inventory::path_doc(__path_get_llm_key),
        crate::controllers::route_inventory::path_doc(__path_delete_llm_key),
    ]
}

#[cfg(feature = "openapi")]
pub(crate) fn gated_openapi_docs() -> Vec<crate::controllers::route_inventory::RouteDoc> {
    vec![crate::controllers::route_inventory::path_doc(
        __path_infer_fill,
    )]
}

#[cfg(feature = "openapi")]
pub(crate) fn job_status_openapi_docs() -> Vec<crate::controllers::route_inventory::RouteDoc> {
    vec![
        crate::controllers::route_inventory::path_doc(__path_infer_job_status),
        crate::controllers::route_inventory::path_doc(__path_list_proposals),
        crate::controllers::route_inventory::path_doc(__path_confirm_proposals),
        crate::controllers::route_inventory::path_doc(__path_reject_proposals),
        crate::controllers::route_inventory::path_doc(__path_discard_proposals),
    ]
}
