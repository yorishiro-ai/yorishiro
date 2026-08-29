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
use uuid::Uuid;

use crate::ee::models::entity_fill;
use crate::ee::models::llm_keys::{self, LlmKeyDescription};
use crate::ee::services::authz;
use crate::ee::services::inference::InferenceClient;

/// Base's own extractors enforce a minimum scope by type; without them here, the check is written out explicitly.
#[derive(Debug, Deserialize)]
pub struct SetLlmKeyRequest {
    /// An OpenAI-compatible chat-completions endpoint, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    pub model: String,
    /// Stored as given and never returned.
    /// `GET` reports only that one is configured.
    pub api_key: String,
}

/// `PUT /hosted/workspace/llm-key`
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

/// `GET /hosted/workspace/llm-key`
async fn get_llm_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<LlmKeyDescription>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let described = llm_keys::describe(&ctx.db, auth_ctx.workspace_id)
        .await?
        .ok_or_else(|| YorishiroError::not_found("no LLM credentials configured"))?;
    Ok(Json(described))
}

/// `DELETE /hosted/workspace/llm-key`
async fn delete_llm_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    llm_keys::clear(&ctx.db, auth_ctx.workspace_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
pub struct InferFillReport {
    /// Groups every snapshot this run takes, so `POST /api/migration-jobs/{job_id}/undo` (base's own, unchanged) can put every entity this run touched back to what it held before.
    pub job_id: Uuid,
    /// Fields a model proposed and this run wrote to `content_entities`.
    pub applied: i64,
    /// Entities skipped: nothing missing, the model declined to guess, or the guess didn't fit the schema.
    pub skipped: i64,
}

/// `POST /hosted/schemas/active/{name}/infer-fill`
///
/// Writes each accepted guess straight to `content_entities`, the same "compute and write immediately" shape the embedding-sync worker types use, in place of the earlier propose/confirm workflow: a guess is reversible the same way any other entity write is, through base's own `content_entities::snapshot`/`undo_job`, so holding it in a separate table pending a second request added a step with no reversibility this deployment didn't already have another way.
async fn infer_fill(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<InferFillReport>, ApiError> {
    // The server calls an LLM here, which is the definition of a paid feature. The licence check is
    // `app::licence_gate`, a layer on this route's own group: it runs before authentication, so an
    // unlicensed deployment answers the same 404 to everyone rather than confirming to a valid key
    // that the endpoint exists and is merely locked.
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    let workspace_id = auth_ctx.workspace_id;

    // Refuse before doing any work: a caller with no key gets one clear error rather than a scan that reports zero applied and reads as "nothing to infer".
    let config = llm_keys::get(&ctx.db, workspace_id).await?.ok_or_else(|| {
        YorishiroError::ValidationFailed {
            message: "this workspace has no LLM credentials configured".into(),
            details: vec![],
            hint: "PUT /hosted/workspace/llm-key".into(),
        }
    })?;

    let db = ctx
        .shared_store
        .get::<crate::db::DbHandle>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
    let schema_txn = db
        .tenant
        .begin_for_workspace(auth_ctx.tenant_id, workspace_id)
        .await
        .internal()?;

    let active =
        crate::models::content_schemas::get_active_schema(&schema_txn, workspace_id, &name).await?;
    let job_id = Uuid::new_v4();
    let client = InferenceClient::new(config);

    let mut applied = 0i64;
    let mut skipped = 0i64;

    // The same set base's fill-defaults would walk: entities on a version older than the active one.
    let rows =
        entity_fill::entities_on_outdated_schema(&schema_txn, workspace_id, &name, active.id)
            .await?;

    for row in rows {
        let Some(type_def) = active.definition.entity_types.get(&row.entity_type) else {
            skipped += 1;
            continue;
        };

        let missing: Vec<&str> = type_def
            .fields
            .keys()
            .filter(|field| row.data.get(field.as_str()).is_none())
            .map(|field| field.as_str())
            .collect();

        if missing.is_empty() {
            skipped += 1;
            continue;
        }

        let answers = client.propose_fields(&row.data, &missing).await?;
        if answers.is_empty() {
            skipped += 1;
            continue;
        }
        let field_count = answers.len() as i64;

        if entity_fill::apply_answers(&schema_txn, workspace_id, &row, job_id, answers).await? {
            applied += field_count;
        } else {
            skipped += 1;
        }
    }

    schema_txn.commit().await.internal()?;

    Ok(Json(InferFillReport {
        job_id,
        applied,
        skipped,
    }))
}

/// Managing the workspace's own LLM credentials.
///
/// Separate from [`gated_routes`] because the licence gate applies per `Routes` and these three are
/// not gated: storing a credential does not require a licence, only spending it on an inference call
/// does. Merging the two groups would extend the gate over these, which is a product decision rather
/// than a routing detail.
///
/// The `hosted` prefix is repeated rather than factored out, so both groups serve the paths a client
/// expects.
pub fn routes() -> Routes {
    Routes::new().prefix("hosted").add(
        "/workspace/llm-key",
        axum::routing::put(set_llm_key)
            .get(get_llm_key)
            .delete(delete_llm_key),
    )
}

/// The route this module's licence gate covers: the server calls an LLM here, which is what makes
/// it a paid feature. See [`routes`] for why the two groups are separate.
pub fn gated_routes() -> Routes {
    Routes::new().prefix("hosted").add(
        "/schemas/active/{name}/infer-fill",
        axum::routing::post(infer_fill),
    )
}
