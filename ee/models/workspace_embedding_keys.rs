//! A workspace's own embedding provider assignment, for pointing a tenant at a different compute backend than the deployment default.
//!
//! Reads and writes go through `ctx.db` (the migration-role connection), not the RLS-scoped tenant pool: `yorishiro_app` has no GRANT on this table, matching `workspace_llm_keys`.
//!
//! A missing assignment leaves the deployment-provider fallback to the caller.

use std::sync::Arc;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_embedding_keys::{ActiveModel, Column, Entity};
use crate::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;
use uuid::Uuid;

pub(crate) async fn configure(
    ctx: &loco_rs::app::AppContext,
    workspace_id: Uuid,
    request: crate::edition::ee::dtos::embedding::SetEmbeddingKeyRequest,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(&ctx.db).await.internal()?;
    let workspace = crate::models::workspace_workspaces::get_workspace(&txn, workspace_id).await?;
    let outcome = set(
        &txn,
        workspace_id,
        &request.base_url,
        &request.model,
        &request.api_key,
        request.dimensions,
        request.send_dimensions_param,
        workspace.embedding_dimensions,
    )
    .await?;
    if matches!(outcome, SetOutcome::WidthChanged { .. }) {
        crate::workers::reindex::enqueue_reindex(ctx, workspace_id, Some(&txn))
            .await
            .internal()?;
    }
    txn.commit().await.internal()?;
    Ok(())
}

/// Builds the workspace's assigned provider without loading the deployment default.
pub(crate) async fn resolve(
    conn: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Option<Arc<dyn EmbeddingProvider>>, YorishiroError> {
    let Some(config) = get(conn, workspace_id).await? else {
        return Ok(None);
    };
    Ok(Some(Arc::new(OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig {
            base_url: config.base_url,
            api_key: config.api_key,
            model: config.model,
            dimensions: config.dimensions as usize,
            send_dimensions_param: config.send_dimensions_param,
        },
    ))))
}

/// What a workspace has configured, without the key itself.
///
/// `api_key` is deliberately absent rather than masked, matching `workspace_llm_keys::LlmKeyDescription`: a masked value still travels through logs and proxies, and nothing a caller does needs it back.
#[derive(Clone, Serialize, sea_orm::FromQueryResult)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = EmbeddingKeyResponse))]
pub(crate) struct EmbeddingKeyDescription {
    pub(crate) base_url: String,
    pub(crate) model: String,
    pub(crate) dimensions: i32,
    /// Always true when present: the row cannot exist without a key.
    /// Callers use the absence of the whole description to mean "not configured".
    pub(crate) configured: bool,
}

/// A workspace's own embedding credentials and model, as the provider lookup reads them to build a provider.
pub(crate) struct EmbeddingKeyConfig {
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    pub(crate) model: String,
    pub(crate) dimensions: i32,
    pub(crate) send_dimensions_param: bool,
}

/// Outcome of storing an embedding key assignment.
///
/// `WidthChanged` is returned when the new provider's width differs from the
/// workspace's existing vectors: the key is stored successfully, but the caller
/// must trigger a reindex so the workspace's entities are re-embedded with the new width.
#[derive(Clone)]
pub enum SetOutcome {
    /// The key was stored and the workspace's width matches the new provider.
    Stored,
    /// The key was stored, but the workspace's existing vectors are at a different width.
    /// The caller should enqueue a reindex for this workspace.
    WidthChanged {
        expected_dimensions: i32,
        new_dimensions: i32,
    },
}

/// Stores or replaces a workspace's own embedding provider assignment.
///
/// `expected_dimensions` is the workspace's own stamped `workspace_workspaces.embedding_dimensions`,
/// or the deployment default's width for a workspace carrying no stamp.
/// When the new provider's width differs, the key is stored and `SetOutcome::WidthChanged` is
/// returned so the caller can enqueue a reindex (the same `reindex_scheduler` enqueue path that
/// `#307` uses). This is preferable to rejecting outright: the operator's intent is to switch
/// widths, and reindexing is the mechanism that makes the switch consistent.
///
/// **Supported widths**: the width must have an embedding table, which the community edition's migrations create.
/// `dimensions` is checked against the community edition's own list, so a width no table exists for is refused here instead of failing on the first entity write.
#[allow(clippy::too_many_arguments)]
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn set(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    base_url: &str,
    model: &str,
    api_key: &str,
    dimensions: i32,
    send_dimensions_param: bool,
    expected_dimensions: Option<i32>,
) -> Result<SetOutcome, YorishiroError> {
    let base_url = crate::edition::ee::data::provider::validate_credentials(base_url, api_key)?;

    if dimensions <= 0 {
        return Err(YorishiroError::ValidationFailed {
            message: "dimensions must be a positive integer".into(),
            details: vec![],
            hint: "set it to the embedding model's own output width".into(),
        });
    }
    let outcome = if let Some(expected) = expected_dimensions
        && expected != dimensions
    {
        SetOutcome::WidthChanged {
            expected_dimensions: expected,
            new_dimensions: dimensions,
        }
    } else {
        SetOutcome::Stored
    };

    let active = ActiveModel {
        workspace_id: ActiveValue::Set(workspace_id),
        base_url: ActiveValue::Set(base_url.to_string()),
        model: ActiveValue::Set(model.to_string()),
        api_key: ActiveValue::Set(api_key.to_string()),
        dimensions: ActiveValue::Set(dimensions),
        send_dimensions_param: ActiveValue::Set(send_dimensions_param),
        updated_at: ActiveValue::Set(crate::time::now().into()),
        ..Default::default()
    };
    Entity::insert(active)
        .on_conflict(
            OnConflict::column(Column::WorkspaceId)
                .update_columns([
                    Column::BaseUrl,
                    Column::Model,
                    Column::ApiKey,
                    Column::Dimensions,
                    Column::SendDimensionsParam,
                    Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(conn)
        .await
        .internal()?;
    Ok(outcome)
}

/// Removes a workspace's own assignment.
/// It falls back to the deployment default afterward.
pub(crate) async fn clear(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    Entity::delete_many()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// What is configured, for an endpoint to report.
/// Never includes the key.
pub(crate) async fn describe(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<EmbeddingKeyDescription>, YorishiroError> {
    Entity::find()
        .select_only()
        .columns([Column::BaseUrl, Column::Model, Column::Dimensions])
        .expr_as(sea_orm::sea_query::Expr::value(true), "configured")
        .filter(Column::WorkspaceId.eq(workspace_id))
        .into_model::<EmbeddingKeyDescription>()
        .one(conn)
        .await
        .internal()
}

/// Resolves the workspace's provider identity without selecting its secret key.
pub(crate) async fn metadata(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<crate::services::embedding::EmbeddingMetadata>, YorishiroError> {
    Ok(describe(conn, workspace_id).await?.map(|description| {
        crate::services::embedding::EmbeddingMetadata {
            model: description.model,
            dimensions: description.dimensions as usize,
        }
    }))
}

/// The credentials themselves, for building a provider.
/// `None` means the workspace has configured none, which the provider lookup reads as "fall back to the deployment default".
pub(crate) async fn get(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<EmbeddingKeyConfig>, YorishiroError> {
    let row = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .one(conn)
        .await
        .internal()?;

    Ok(row.map(|row| EmbeddingKeyConfig {
        base_url: row.base_url,
        api_key: row.api_key,
        model: row.model,
        dimensions: row.dimensions,
        send_dimensions_param: row.send_dimensions_param,
    }))
}
