//! Inferring values for fields an entity is missing (§FR-8-2 mode B), and the per-workspace
//! credentials it runs on.
//!
//! This product does not pay for inference (requirements §1.3), so a workspace brings its own
//! key. A workspace with none configured gets a 422 rather than a fall back to `default` values
//! -- a caller who asked for inference and silently received defaults would have no way to tell
//! that nothing was inferred.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;
use yorishiro_core::ResultExt;
use yorishiro_core::error::YorishiroError;
use yorishiro_core::repositories::fill_proposals::{self, ConfirmReport, FillProposal};
use yorishiro_core::repositories::llm_keys::{self, LlmKeyDescription};
use yorishiro_core::repositories::schemas;
use yorishiro_core::services::inference::InferenceClient;

use crate::error::ApiError;
use crate::http::middleware::auth::{Authorized, ReadScope, SchemaScope};
use crate::state::AppState;

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetLlmKeyRequest {
    /// An OpenAI-compatible chat-completions endpoint, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    pub model: String,
    /// Stored as given and never returned. `GET` reports only that one is configured.
    pub api_key: String,
}

#[utoipa::path(
    put,
    path = "/api/workspace/llm-key",
    request_body = SetLlmKeyRequest,
    responses(
        (status = 204, description = "Stored"),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 422, description = "Empty key", body = crate::error::ApiErrorBody),
    ),
    tag = "inference",
)]
pub async fn set_llm_key(
    State(state): State<AppState>,
    authorized: Authorized<SchemaScope>,
    Json(request): Json<SetLlmKeyRequest>,
) -> Result<StatusCode, ApiError> {
    llm_keys::set(
        &state.identity_pool,
        authorized.ctx.workspace_id,
        &request.base_url,
        &request.model,
        &request.api_key,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/workspace/llm-key",
    responses(
        (status = 200, description = "What is configured, without the key", body = LlmKeyDescription),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 404, description = "Nothing configured", body = crate::error::ApiErrorBody),
    ),
    tag = "inference",
)]
pub async fn get_llm_key(
    State(state): State<AppState>,
    authorized: Authorized<ReadScope>,
) -> Result<Json<LlmKeyDescription>, ApiError> {
    let described = llm_keys::describe(&state.identity_pool, authorized.ctx.workspace_id)
        .await?
        .ok_or_else(|| YorishiroError::not_found("no LLM credentials configured"))?;
    Ok(Json(described))
}

#[utoipa::path(
    delete,
    path = "/api/workspace/llm-key",
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
    ),
    tag = "inference",
)]
pub async fn delete_llm_key(
    State(state): State<AppState>,
    authorized: Authorized<SchemaScope>,
) -> Result<StatusCode, ApiError> {
    llm_keys::clear(&state.identity_pool, authorized.ctx.workspace_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, serde::Serialize, ToSchema)]
pub struct InferFillReport {
    /// Groups the proposals, and later the snapshots a confirmation takes.
    pub job_id: Uuid,
    /// How many fields the model proposed a value for.
    pub proposed: i64,
    /// Entities the model declined to guess for, or that had nothing missing.
    pub skipped: i64,
}

#[utoipa::path(
    post,
    path = "/api/schemas/active/{name}/infer-fill",
    params(("name" = String, Path, description = "Schema name")),
    responses(
        (status = 200, description = "What was proposed, awaiting confirmation", body = InferFillReport),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 422, description = "No LLM credentials configured for this workspace", body = crate::error::ApiErrorBody),
    ),
    tag = "inference",
)]
pub async fn infer_fill(
    State(state): State<AppState>,
    mut authorized: Authorized<SchemaScope>,
    Path(name): Path<String>,
) -> Result<Json<InferFillReport>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;

    // Refuse before doing any work: a caller with no key gets one clear error rather than a
    // scan that reports zero proposals and reads as "nothing to infer".
    let config = llm_keys::get(&state.identity_pool, workspace_id)
        .await?
        .ok_or_else(|| YorishiroError::ValidationFailed {
            message: "this workspace has no LLM credentials configured".into(),
            details: vec![],
            hint: "PUT /api/workspace/llm-key, or use fill-defaults instead".into(),
        })?;

    let active = schemas::get_active_schema(authorized.conn(), workspace_id, &name).await?;
    let job_id = Uuid::new_v4();
    let client = InferenceClient::new(config);

    let mut proposed = 0i64;
    let mut skipped = 0i64;

    // The same set `fill-defaults` walks: entities on a version older than the active one. An
    // entity already on the active version has nothing the schema says is missing.
    let rows: Vec<(Uuid, String, serde_json::Value)> = sqlx::query_as(
        "SELECT e.id, e.entity_type, e.data \
         FROM content.entities e \
         JOIN content.schemas s ON s.id = e.schema_id \
         WHERE e.workspace_id = $1 AND s.name = $2 AND e.schema_id <> $3",
    )
    .bind(workspace_id)
    .bind(&name)
    .bind(active.id)
    .fetch_all(authorized.conn())
    .await
    .internal()?;

    for (entity_id, entity_type, data) in rows {
        let Some(type_def) = active.definition.entity_types.get(&entity_type) else {
            skipped += 1;
            continue;
        };

        let missing: Vec<&str> = type_def
            .fields
            .keys()
            .filter(|field| data.get(field.as_str()).is_none())
            .map(|field| field.as_str())
            .collect();

        if missing.is_empty() {
            skipped += 1;
            continue;
        }

        let answers = client.propose_fields(&data, &missing).await?;
        if answers.is_empty() {
            skipped += 1;
            continue;
        }

        for (field, value) in answers {
            fill_proposals::record(
                authorized.conn(),
                workspace_id,
                job_id,
                entity_id,
                &field,
                &value,
            )
            .await?;
            proposed += 1;
        }
    }

    Ok(Json(InferFillReport {
        job_id,
        proposed,
        skipped,
    }))
}

#[utoipa::path(
    get,
    path = "/api/migration-jobs/{job_id}/proposals",
    params(("job_id" = Uuid, Path, description = "Job id from an infer-fill run")),
    responses(
        (status = 200, description = "What the model proposed", body = Vec<FillProposal>),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
    ),
    tag = "inference",
)]
pub async fn list_proposals(
    mut authorized: Authorized<ReadScope>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Vec<FillProposal>>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let proposals = fill_proposals::for_job(authorized.conn(), workspace_id, job_id).await?;
    Ok(Json(proposals))
}

#[utoipa::path(
    post,
    path = "/api/migration-jobs/{job_id}/confirm",
    params(("job_id" = Uuid, Path, description = "Job id from an infer-fill run")),
    responses(
        (status = 200, description = "What was applied", body = ConfirmReport),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "No proposals for that job", body = crate::error::ApiErrorBody),
    ),
    tag = "inference",
)]
pub async fn confirm_proposals(
    mut authorized: Authorized<SchemaScope>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<ConfirmReport>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let report = fill_proposals::confirm(authorized.conn(), workspace_id, job_id).await?;
    Ok(Json(report))
}

#[cfg(test)]
#[path = "../../../tests/http/controllers/inference.rs"]
mod tests;
