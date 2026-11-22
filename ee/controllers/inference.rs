//! Inferring values for fields an entity is missing, and the per-workspace credentials it runs on.
//!
//! This product does not pay for inference, so a workspace brings its own key.
//! A workspace with none configured gets a 422 rather than a fall back to `default` values: a caller who asked for inference and silently received defaults would have no way to tell that nothing was inferred.

use crate::controllers::ApiError;
use crate::controllers::middleware::auth::require_scope;
use crate::controllers::openapi::Routes;
use crate::error::YorishiroError;
use crate::models::api_keys::ApiKeyScope;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;

use crate::edition::ee::controllers::middleware::auth as authz;
use crate::edition::ee::dtos::inference::{
    InferFillResponse, InferFillStatus, InferJobStatusResponse, SetLlmKeyRequest,
};
use crate::edition::ee::models::inference_jobs;
use crate::edition::ee::models::inference_jobs::InferenceJobStatus;
use crate::edition::ee::models::inference_proposals;
use crate::edition::ee::models::workspace_llm_keys;

/// `POST /api/schemas/active/{name}/infer-fill`
///
/// Enqueues an async infer-fill job whose answers are stored as reviewable proposals.
/// Requires `inference_create` on the selected workspace in addition to schema scope.
/// The worker rechecks the credential and IAM policy before inference and proposal storage.
///
/// Poll for completion via `GET /api/inference-jobs/{job_id}`.
async fn infer_fill(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(crate::dtos::common::NamePath { name }): Path<crate::dtos::common::NamePath<String>>,
) -> Result<Json<InferFillResponse>, ApiError> {
    // Gate before authentication so unlicensed requests all receive the same 404.
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    let job_id =
        crate::edition::ee::workers::infer_fill::enqueue_infer_fill(&ctx, &auth_ctx, name).await?;

    Ok(Json(InferFillResponse {
        job_id,
        status: InferFillStatus::Queued,
    }))
}

/// `GET /api/inference-jobs/{job_id}`
///
/// Poll for the durable result of an infer-fill job.
///
/// Requires authentication and read scope, and only returns a job belonging to the
/// authenticated workspace.
async fn infer_job_status(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(crate::dtos::common::JobIdPath { job_id }): Path<crate::dtos::common::JobIdPath<String>>,
) -> Result<Json<InferJobStatusResponse>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let parsed_job_id = uuid::Uuid::parse_str(&job_id)
        .map_err(|_| YorishiroError::not_found("infer-fill job not found"))?;
    let result = inference_jobs::get_as(&ctx.db, &auth_ctx, parsed_job_id).await?;

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
    Routes::new()
        .tag("enterprise")
        .prefix("api/workspace")
        .add(
            "/llm-key",
            aide::axum::routing::put_with(set_llm_key, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["schema"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["llm_provider_update"]),
                ))
                .response_with::<204, (), _>(|op| op.description("LLM credentials saved"))
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<422, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/llm-key",
            aide::axum::routing::get_with(get_llm_key, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["read"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["llm_provider_get"]),
                ))
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/llm-key",
            aide::axum::routing::delete_with(delete_llm_key, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["schema"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["llm_provider_delete"]),
                ))
                .response_with::<204, (), _>(|op| op.description("LLM credentials removed"))
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
}

/// The route this module's licence gate covers: the server calls an LLM here, which is what makes
/// it an enterprise feature. See [`routes`] for why the two groups are separate.
pub fn gated_routes() -> Routes {
    Routes::new().tag("enterprise").prefix("api/schemas").add(
        "/active/{name}/infer-fill",
        aide::axum::routing::post_with(infer_fill, |op| {
            op.with(crate::controllers::openapi::extension(
                "x-yorishiro-required-scopes",
                serde_json::json!(["schema"]),
            ))
            .with(crate::controllers::openapi::extension(
                "x-yorishiro-required-actions",
                serde_json::json!(["inference_create"]),
            ))
            .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
            .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
            .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            .response::<422, Json<crate::controllers::openapi::ApiErrorBody>>()
            .response::<503, Json<crate::controllers::openapi::ApiErrorBody>>()
        }),
    )
}

/// `GET /api/inference-jobs/{job_id}`
///
/// Poll for the result of an infer-fill job. Follows the same pattern as
/// `migration_routes()` (`api/migration-jobs/{job_id}/undo`): a dedicated prefix for
/// async-job polling, under the licence gate because results include workspace
/// activity counts that reveal whether a workspace has data to infer.
pub fn inference_job_status_routes() -> Routes {
    Routes::new()
        .tag("enterprise")
        .prefix("api/inference-jobs")
        .add(
            "/{job_id}",
            aide::axum::routing::get_with(infer_job_status, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["read"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["inference_job_get"]),
                ))
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{job_id}/proposals",
            aide::axum::routing::get_with(list_proposals, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["read"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["inference_proposal_list"]),
                ))
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{job_id}/confirm",
            aide::axum::routing::post_with(confirm_proposals, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["schema"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["inference_confirm"]),
                ))
                .response::<202, Json<crate::dtos::jobs::QueuedJobResponse>>()
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<409, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{job_id}/reject",
            aide::axum::routing::post_with(reject_proposals, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["schema"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["inference_reject"]),
                ))
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{job_id}/discard",
            aide::axum::routing::post_with(discard_proposals, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["schema"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["inference_discard"]),
                ))
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/confirmations/{job_id}",
            aide::axum::routing::get_with(confirmation_status, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["schema"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["inference_confirmation_get"]),
                ))
                .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
}

async fn confirmation_status(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(crate::dtos::common::JobIdPath { job_id }): Path<
        crate::dtos::common::JobIdPath<uuid::Uuid>,
    >,
) -> Result<Json<crate::models::queue_job_lifecycles::BulkJobStatus>, ApiError> {
    let auth = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth, ApiKeyScope::Schema)?;
    Ok(Json(
        inference_jobs::confirmation_status_as(&ctx.db, &auth, job_id).await?,
    ))
}

fn parse_job_id(job_id: &str) -> Result<uuid::Uuid, YorishiroError> {
    uuid::Uuid::parse_str(job_id).map_err(|_| YorishiroError::not_found("infer-fill job not found"))
}

/// `GET /api/inference-jobs/{job_id}/proposals`
async fn list_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(crate::dtos::common::JobIdPath { job_id }): Path<crate::dtos::common::JobIdPath<String>>,
) -> Result<Json<Vec<inference_proposals::ProposalRecord>>, ApiError> {
    let auth = authz::authenticate_workspace(&ctx, &headers).await?;
    Ok(Json(
        inference_proposals::for_job_as(&ctx.db, &auth, parse_job_id(&job_id)?).await?,
    ))
}

async fn reject_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(crate::dtos::common::JobIdPath { job_id }): Path<crate::dtos::common::JobIdPath<String>>,
) -> Result<Json<inference_proposals::ProposalActionReport>, ApiError> {
    let auth = authz::authenticate_workspace(&ctx, &headers).await?;
    Ok(Json(
        inference_proposals::reject_as(&ctx.db, &auth, parse_job_id(&job_id)?).await?,
    ))
}

async fn discard_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(crate::dtos::common::JobIdPath { job_id }): Path<crate::dtos::common::JobIdPath<String>>,
) -> Result<Json<inference_proposals::ProposalActionReport>, ApiError> {
    let auth = authz::authenticate_workspace(&ctx, &headers).await?;
    Ok(Json(
        inference_proposals::discard_as(&ctx.db, &auth, parse_job_id(&job_id)?).await?,
    ))
}

/// Enqueues confirmation with `inference_confirm` on the selected workspace and schema scope.
/// The worker rechecks the credential and IAM policy before applying proposals.
async fn confirm_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(crate::dtos::common::JobIdPath { job_id }): Path<crate::dtos::common::JobIdPath<String>>,
) -> Result<(StatusCode, Json<crate::dtos::jobs::QueuedJobResponse>), ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    let job_id = parse_job_id(&job_id)?;
    let job_id =
        crate::edition::ee::workers::confirm_proposals::enqueue(&ctx, &auth_ctx, job_id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(crate::dtos::jobs::QueuedJobResponse { job_id }),
    ))
}

/// `PUT /api/workspace/llm-key`
async fn set_llm_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(body): Json<SetLlmKeyRequest>,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    workspace_llm_keys::set_as(
        &ctx.db,
        &auth_ctx,
        &body.base_url,
        &body.model,
        &body.api_key,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/workspace/llm-key`
async fn get_llm_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<workspace_llm_keys::LlmKeyDescription>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let described = workspace_llm_keys::describe_as(&ctx.db, &auth_ctx)
        .await?
        .ok_or_else(|| YorishiroError::not_found("no LLM credentials configured"))?;
    Ok(Json(described))
}

/// `DELETE /api/workspace/llm-key`
async fn delete_llm_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    workspace_llm_keys::clear_as(&ctx.db, &auth_ctx).await?;
    Ok(StatusCode::NO_CONTENT)
}
