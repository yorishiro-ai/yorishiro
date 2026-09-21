//! Inferring values for fields an entity is missing, and the per-workspace credentials it runs on.
//!
//! This product does not pay for inference, so a workspace brings its own key.
//! A workspace with none configured gets a 422 rather than a fall back to `default` values: a caller who asked for inference and silently received defaults would have no way to tell that nothing was inferred.

use crate::controllers::ApiError;
use crate::error::{ResultExt, YorishiroError};
use crate::services::auth::{ApiKeyScope, require_scope};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::{Deserialize, Serialize};

use crate::ee::models::inference_jobs;
use crate::ee::models::llm_keys;
use crate::ee::services::authz;

/// `POST /api/schemas/active/{name}/infer-fill`
///
/// Enqueues an async infer-fill job. The same "compute and write immediately" shape the
/// embedding-sync worker types use — accepted guesses land straight into `entity_entities`,
/// reversible through `POST /api/migration-jobs/{job_id}/undo`.
///
/// Poll for completion via `GET /api/inference-jobs/{job_id}`.
#[derive(Debug, Serialize)]
pub struct InferFillRequest {
    /// The durable job ID used by the polling endpoint.
    pub job_id: String,
    /// Initial status: always `"queued"`.
    pub status: String,
}

async fn infer_fill(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<InferFillRequest>, ApiError> {
    // The server calls an LLM here, which is the definition of an enterprise feature. The licence check is
    // `app::licence_gate`, a layer on this route's own group: it runs before authentication, so an
    // unlicensed deployment answers the same 404 to everyone rather than confirming to a valid key
    // that the endpoint exists and is merely locked.
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    let workspace_id = auth_ctx.workspace_id;

    // Refuse before doing any work: a caller with no key gets one clear error rather than a scan that reports zero applied and reads as "nothing to infer".
    if llm_keys::get(&ctx.db, workspace_id)
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

    Ok(Json(InferFillRequest {
        job_id,
        status: "queued".to_string(),
    }))
}

/// `GET /api/inference-jobs/{job_id}`
///
/// Poll for the durable result of an infer-fill job.
///
/// Requires authentication and read scope, and only returns a job belonging to the
/// authenticated workspace.
#[derive(Debug, Serialize)]
pub struct InferJobStatus {
    /// The job ID.
    pub job_id: String,
    /// One of `queued`, `running`, `completed`, `failed`.
    pub status: String,
    /// Fields the model proposed and wrote to `entity_entities`. Only present on `completed`.
    pub applied: Option<i64>,
    /// Entities skipped: nothing missing, the model declined to guess, or the guess didn't fit. Only present on `completed`.
    pub skipped: Option<i64>,
    /// Error message if the job failed.
    pub error: Option<String>,
}

async fn infer_job_status(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<InferJobStatus>, ApiError> {
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
    Ok(Json(InferJobStatus {
        job_id,
        status: status.clone(),
        applied: (status == inference_jobs::COMPLETED).then_some(result.applied),
        skipped: (status == inference_jobs::COMPLETED).then_some(result.skipped),
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
}

/// `PUT /api/workspace/llm-key`
async fn set_llm_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(body): Json<SetLlmKeyRequest>,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    llm_keys::set(
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
async fn get_llm_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<llm_keys::LlmKeyDescription>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let described = llm_keys::describe(&ctx.db, auth_ctx.workspace_id)
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
    llm_keys::clear(&ctx.db, auth_ctx.workspace_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct SetLlmKeyRequest {
    /// An OpenAI-compatible chat-completions endpoint, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    pub model: String,
    /// Stored as given and never returned.
    /// `GET` reports only that one is configured.
    pub api_key: String,
}

impl std::fmt::Debug for SetLlmKeyRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetLlmKeyRequest")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_llm_api_key() {
        let request = SetLlmKeyRequest {
            base_url: "https://provider.example".into(),
            model: "model".into(),
            api_key: "do-not-render-llm-value".into(),
        };

        let rendered = format!("{request:?}");

        assert!(!rendered.contains("do-not-render-llm-value"));
        assert!(rendered.contains("provider.example"));
    }
}
