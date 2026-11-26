//! Inferring values for fields an entity is missing, and the per-workspace credentials it runs on.
//!
//! This product does not pay for inference, so a workspace brings its own key.
//! A workspace with none configured gets a 422 rather than a fall back to `default` values: a caller who asked for inference and silently received defaults would have no way to tell that nothing was inferred.

use crate::controllers::Accepted;
use crate::controllers::ApiError;
use crate::controllers::openapi::Routes;
use crate::edition::ee::controllers::middleware::auth::WorkspaceAuthenticated;
use crate::error::OptionExt;
use crate::error::YorishiroError;
use axum::Json;
use axum::extract::{Path, State};
use loco_rs::app::AppContext;

use crate::edition::ee::dtos::inference::{
    InferFillResponse, InferFillStatus, InferJobStatusResponse, SetLlmKeyRequest,
};
use crate::edition::ee::models::inference_jobs;
use crate::edition::ee::models::inference_jobs::InferenceJobStatus;
use crate::edition::ee::models::inference_proposals;
use crate::edition::ee::models::workspace_llm_keys;

/// `POST /api/schemas/active/{schema_name}/infer-fill`
///
/// Enqueues an async infer-fill job whose answers are stored as reviewable proposals.
/// Requires `inference_create` on the selected workspace.
/// The worker rechecks the credential and IAM policy before inference and proposal storage.
///
/// Poll `GET /api/inference-jobs/{inference_job_id}` for completion.
async fn infer_fill(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth_ctx): WorkspaceAuthenticated,
    Path(crate::dtos::common::SchemaNamePath { schema_name }): Path<
        crate::dtos::common::SchemaNamePath,
    >,
) -> Result<Json<InferFillResponse>, ApiError> {
    // Gate before authentication so unlicensed requests all receive the same 404.
    let job_id =
        crate::edition::ee::workers::infer_fill::enqueue_infer_fill(&ctx, &auth_ctx, schema_name)
            .await?;

    Ok(Json(InferFillResponse {
        inference_job_id: job_id,
        status: InferFillStatus::Queued,
    }))
}

/// `GET /api/inference-jobs/{inference_job_id}`
///
/// Poll for the durable result of an infer-fill job.
///
/// Requires authentication, and only returns a job belonging to the
/// authenticated workspace.
async fn infer_job_status(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth_ctx): WorkspaceAuthenticated,
    Path(crate::dtos::common::InferenceJobIdPath {
        inference_job_id: job_id,
    }): Path<crate::dtos::common::InferenceJobIdPath>,
) -> Result<Json<InferJobStatusResponse>, ApiError> {
    let parsed_job_id = uuid::Uuid::parse_str(&job_id)
        .map_err(|_| YorishiroError::not_found("infer-fill job not found"))?;
    let result = inference_jobs::get_as(&ctx.db, &auth_ctx, parsed_job_id).await?;

    let status = result.status;
    let completed = status == InferenceJobStatus::Completed;
    Ok(Json(InferJobStatusResponse {
        inference_job_id: job_id,
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
            "/llm-provider",
            aide::axum::routing::put_with(set_llm_key, |op| op.id("UpdateWorkspaceLlmProvider")),
        )
        .add(
            "/llm-provider",
            aide::axum::routing::get_with(get_llm_key, |op| op.id("GetWorkspaceLlmProvider")),
        )
        .add(
            "/llm-provider",
            aide::axum::routing::delete_with(delete_llm_key, |op| {
                op.id("DeleteWorkspaceLlmProvider")
            }),
        )
}

/// The route this module's licence gate covers: the server calls an LLM here, which is what makes
/// it an enterprise feature. See [`routes`] for why the two groups are separate.
pub fn gated_routes() -> Routes {
    Routes::new().tag("enterprise").prefix("api/schemas").add(
        "/active/{schema_name}/infer-fill",
        aide::axum::routing::post_with(infer_fill, |op| op.id("CreateInferenceJob")),
    )
}

/// `GET /api/inference-jobs/{inference_job_id}`
///
/// Poll for the result of an infer-fill job. Follows the same pattern as
/// `migration_routes()` (`api/migration-jobs/{migration_job_id}:undo`): a dedicated prefix for
/// async-job polling, under the licence gate because results include workspace
/// activity counts that reveal whether a workspace has data to infer.
pub fn inference_job_status_routes() -> Routes {
    Routes::new()
        .tag("enterprise")
        .prefix("api/inference-jobs")
        .add(
            "/{inference_job_id}",
            aide::axum::routing::get_with(infer_job_status, |op| op.id("GetInferenceJob")),
        )
        .add(
            "/{inference_job_id}/proposals",
            aide::axum::routing::get_with(list_proposals, |op| op.id("ListInferenceProposals")),
        )
        .add(
            "/{inference_job_id}/confirm",
            aide::axum::routing::post_with(confirm_proposals, |op| {
                op.id("ConfirmInferenceProposals")
            }),
        )
        .add(
            "/{inference_job_id}/discard",
            aide::axum::routing::post_with(discard_proposals, |op| {
                op.id("DiscardInferenceProposals")
            }),
        )
        .add(
            "/confirmations/{confirmation_job_id}",
            aide::axum::routing::get_with(confirmation_status, |op| {
                op.id("GetInferenceConfirmation")
            }),
        )
}

async fn confirmation_status(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth): WorkspaceAuthenticated,
    Path(crate::dtos::common::ConfirmationJobIdPath {
        confirmation_job_id,
    }): Path<crate::dtos::common::ConfirmationJobIdPath>,
) -> Result<Json<crate::edition::ee::dtos::inference::ConfirmationJobStatus>, ApiError> {
    Ok(Json(
        crate::edition::ee::dtos::inference::ConfirmationJobStatus {
            confirmation_job_id,
            status: inference_jobs::confirmation_status_as(&ctx.db, &auth, confirmation_job_id)
                .await?,
        },
    ))
}

fn parse_job_id(job_id: &str) -> Result<uuid::Uuid, YorishiroError> {
    uuid::Uuid::parse_str(job_id).map_err(|_| YorishiroError::not_found("infer-fill job not found"))
}

/// `GET /api/inference-jobs/{inference_job_id}/proposals`
async fn list_proposals(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth): WorkspaceAuthenticated,
    Path(crate::dtos::common::InferenceJobIdPath {
        inference_job_id: job_id,
    }): Path<crate::dtos::common::InferenceJobIdPath>,
) -> Result<Json<Vec<inference_proposals::ProposalRecord>>, ApiError> {
    Ok(Json(
        inference_proposals::for_job_as(&ctx.db, &auth, parse_job_id(&job_id)?).await?,
    ))
}

async fn discard_proposals(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth): WorkspaceAuthenticated,
    Path(crate::dtos::common::InferenceJobIdPath {
        inference_job_id: job_id,
    }): Path<crate::dtos::common::InferenceJobIdPath>,
) -> Result<Json<inference_proposals::ProposalActionReport>, ApiError> {
    Ok(Json(
        inference_proposals::discard_as(&ctx.db, &auth, parse_job_id(&job_id)?).await?,
    ))
}

/// Enqueues confirmation with `inference_confirm` on the selected workspace.
/// The worker rechecks the credential and IAM policy before applying proposals.
async fn confirm_proposals(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth_ctx): WorkspaceAuthenticated,
    Path(crate::dtos::common::InferenceJobIdPath {
        inference_job_id: job_id,
    }): Path<crate::dtos::common::InferenceJobIdPath>,
) -> Result<Accepted<crate::edition::ee::dtos::inference::ConfirmationJobAccepted>, ApiError> {
    let job_id = parse_job_id(&job_id)?;
    let job_id =
        crate::edition::ee::workers::confirm_proposals::enqueue(&ctx, &auth_ctx, job_id).await?;
    Ok(Accepted(
        crate::edition::ee::dtos::inference::ConfirmationJobAccepted {
            confirmation_job_id: job_id,
        },
    ))
}

/// `PUT /api/workspace/llm-provider`
async fn set_llm_key(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth_ctx): WorkspaceAuthenticated,
    Json(body): Json<SetLlmKeyRequest>,
) -> Result<axum::response::NoContent, ApiError> {
    workspace_llm_keys::set_as(
        &ctx.db,
        &auth_ctx,
        &body.base_url,
        &body.model,
        &body.api_key,
    )
    .await?;
    Ok(axum::response::NoContent)
}

/// `GET /api/workspace/llm-provider`
async fn get_llm_key(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth_ctx): WorkspaceAuthenticated,
) -> Result<Json<workspace_llm_keys::LlmKeyDescription>, ApiError> {
    let described = workspace_llm_keys::describe_as(&ctx.db, &auth_ctx)
        .await?
        .or_not_found("no LLM credentials configured")?;
    Ok(Json(described))
}

/// `DELETE /api/workspace/llm-provider`
async fn delete_llm_key(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth_ctx): WorkspaceAuthenticated,
) -> Result<axum::response::NoContent, ApiError> {
    workspace_llm_keys::clear_as(&ctx.db, &auth_ctx).await?;
    Ok(axum::response::NoContent)
}
