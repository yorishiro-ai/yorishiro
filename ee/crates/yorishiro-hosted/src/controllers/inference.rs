//! Inferring values for fields an entity is missing, and the per-workspace credentials it runs
//! on. Ported from master's `ee/crates/yorishiro-hosted/src/http/controllers/inference.rs`.
//!
//! This product does not pay for inference, so a workspace brings its own key. A workspace with
//! none configured gets a 422 rather than a fall back to `default` values: a caller who asked
//! for inference and silently received defaults would have no way to tell that nothing was
//! inferred.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::FromQueryResult;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use yorishiro_core::controllers::ApiError;
use yorishiro_core::error::{ResultExt, YorishiroError};
use yorishiro_core::services::auth::{ApiKeyScope, AuthContext};

use crate::models::fill_proposals::{self, ConfirmReport, FillProposal};
use crate::models::llm_keys::{self, LlmKeyDescription};
use crate::services::authz;
use crate::services::inference::InferenceClient;
use crate::services::licence::LicenceState;

/// Base's own extractors enforce a minimum scope by type. Without them, the check is written
/// out: the ordering on `ApiKeyScope` is the same one they use.
fn require_scope(ctx: &AuthContext, needed: ApiKeyScope) -> Result<(), YorishiroError> {
    if ctx.scope < needed {
        return Err(YorishiroError::ScopeInsufficient {
            message: format!("this endpoint needs the {needed:?} scope or higher"),
            hint: "issue a key with a higher scope".into(),
        });
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct SetLlmKeyRequest {
    /// An OpenAI-compatible chat-completions endpoint, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    pub model: String,
    /// Stored as given and never returned. `GET` reports only that one is configured.
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
    /// Groups the proposals, and later the snapshots a confirmation takes.
    pub job_id: Uuid,
    /// How many fields the model proposed a value for.
    pub proposed: i64,
    /// Entities the model declined to guess for, or that had nothing missing.
    pub skipped: i64,
}

/// `POST /hosted/schemas/active/{name}/infer-fill`
async fn infer_fill(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<InferFillReport>, ApiError> {
    // The server calls an LLM here, which is the definition of a paid feature. Checked before
    // authentication so an unlicensed deployment answers the same 404 to everyone, rather than
    // confirming to a valid key that the endpoint exists and is merely locked.
    ctx.shared_store
        .get::<LicenceState>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("LicenceState missing")))?
        .require_active()?;
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    let workspace_id = auth_ctx.workspace_id;

    // Refuse before doing any work: a caller with no key gets one clear error rather than a
    // scan that reports zero proposals and reads as "nothing to infer".
    let config = llm_keys::get(&ctx.db, workspace_id)
        .await?
        .ok_or_else(|| YorishiroError::ValidationFailed {
            message: "this workspace has no LLM credentials configured".into(),
            details: vec![],
            hint: "PUT /hosted/workspace/llm-key".into(),
        })?;

    let db = ctx
        .shared_store
        .get::<yorishiro_core::db::DbHandle>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
    let schema_txn = db
        .tenant
        .begin_for_workspace(auth_ctx.tenant_id, workspace_id)
        .await
        .internal()?;

    let active =
        yorishiro_core::models::content_schemas::get_active_schema(&schema_txn, workspace_id, &name)
            .await?;
    let job_id = Uuid::new_v4();
    let client = InferenceClient::new(config);

    let mut proposed = 0i64;
    let mut skipped = 0i64;

    // The same set base's fill-defaults would walk: entities on a version older than the
    // active one. An entity already on the active version has nothing the schema says is
    // missing.
    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        id: Uuid,
        entity_type: String,
        data: serde_json::Value,
    }
    let rows: Vec<Row> = Row::find_by_statement(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT e.id, e.entity_type, e.data \
           FROM content_entities e \
           JOIN content_schemas s ON s.id = e.schema_id \
          WHERE e.workspace_id = $1 AND s.name = $2 AND e.schema_id <> $3",
        [workspace_id.into(), name.clone().into(), active.id.into()],
    ))
    .all(&schema_txn)
    .await
    .internal()?;

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

        for (field, value) in answers {
            fill_proposals::record(&schema_txn, workspace_id, job_id, row.id, &field, &value)
                .await?;
            proposed += 1;
        }
    }

    schema_txn.commit().await.internal()?;

    Ok(Json(InferFillReport {
        job_id,
        proposed,
        skipped,
    }))
}

/// `GET /hosted/migration-jobs/{job_id}/proposals`
async fn list_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Vec<FillProposal>>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let db = ctx
        .shared_store
        .get::<yorishiro_core::db::DbHandle>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
    let schema_txn = db
        .tenant
        .begin_for_workspace(auth_ctx.tenant_id, auth_ctx.workspace_id)
        .await
        .internal()?;
    let proposals = fill_proposals::for_job(&schema_txn, auth_ctx.workspace_id, job_id).await?;
    Ok(Json(proposals))
}

/// `POST /hosted/migration-jobs/{job_id}/confirm`
async fn confirm_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<ConfirmReport>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    let db = ctx
        .shared_store
        .get::<yorishiro_core::db::DbHandle>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
    let schema_txn = db
        .tenant
        .begin_for_workspace(auth_ctx.tenant_id, auth_ctx.workspace_id)
        .await
        .internal()?;
    let report = fill_proposals::confirm(&schema_txn, auth_ctx.workspace_id, job_id).await?;
    schema_txn.commit().await.internal()?;
    Ok(Json(report))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("hosted")
        .add(
            "/workspace/llm-key",
            axum::routing::put(set_llm_key)
                .get(get_llm_key)
                .delete(delete_llm_key),
        )
        .add(
            "/schemas/active/{name}/infer-fill",
            axum::routing::post(infer_fill),
        )
        .add(
            "/migration-jobs/{job_id}/proposals",
            axum::routing::get(list_proposals),
        )
        .add(
            "/migration-jobs/{job_id}/confirm",
            axum::routing::post(confirm_proposals),
        )
}
