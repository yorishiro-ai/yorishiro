//! A workspace's own embedding provider assignment, for pointing a tenant at a different compute backend than the deployment default.
//!
//! Reads and writes go through `ctx.db` (the migration-role connection), not the RLS-scoped tenant pool: `yorishiro_app` has no GRANT on this table, matching `workspace_llm_keys`.
//!
//! A missing assignment leaves the deployment-provider fallback to the caller.

use std::sync::Arc;

use super::super::data::provider::{check_client, check_error, validate_credentials};
use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_embedding_keys::{ActiveModel, Column, Entity};
use crate::models::access_keys::AuthContext;
use crate::models::iam::policy::Action;
use crate::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;
use uuid::Uuid;

pub(crate) async fn configure(
    ctx: &loco_rs::app::AppContext,
    actor: &AuthContext,
    request: crate::edition::ee::dtos::embedding::SetEmbeddingKeyRequest,
) -> Result<(), YorishiroError> {
    let workspace_id = actor.require_workspace()?;
    let action = Action::UpdateWorkspaceEmbeddingProvider;
    super::iam::access::workspace_transaction(&ctx.db, actor, action)
        .await?
        .commit()
        .await
        .internal()?;
    let config = OpenAiCompatibleConfig {
        base_url: validate_credentials(&request.base_url, &request.api_key)?.into(),
        model: request.model,
        api_key: request.api_key,
        dimensions: dimension_count(request.dimensions)?,
        send_dimensions_param: request.send_dimensions_param,
        query_instruction: request.query_instruction,
        document_prefix: request.document_prefix,
    };
    let sample = OpenAiCompatibleProvider::with_client(config.clone(), check_client()?)
        .embed_as(
            crate::services::embedding::EmbedKind::Document,
            "Provider check",
        )
        .await
        .map_err(|error| check_error(&config.base_url, error))?;
    crate::models::entity_embeddings::validate_vector(&sample, config.dimensions)?;

    let txn = super::iam::access::workspace_transaction(&ctx.db, actor, action).await?;
    let previous = get(&txn, workspace_id).await?;
    let query_only = previous.as_ref().is_some_and(|old| {
        old.base_url == config.base_url
            && old.model == config.model
            && old.api_key == config.api_key
            && old.dimensions == config.dimensions
            && old.send_dimensions_param == config.send_dimensions_param
            && old.document_prefix == config.document_prefix
    });
    if query_only {
        // A query-only change keeps stored document vectors valid, so it skips the revision bump and the reindex.
        Entity::update_many()
            .col_expr(
                Column::QueryInstruction,
                sea_orm::sea_query::Expr::value(config.query_instruction),
            )
            .col_expr(
                Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(crate::data::time::now()),
            )
            .filter(Column::WorkspaceId.eq(workspace_id))
            .exec(&txn)
            .await
            .internal()?;
        txn.commit().await.internal()?;
        return Ok(());
    }
    save(&txn, workspace_id, config).await?;
    crate::workers::reindex::enqueue_reindex(ctx, workspace_id, Some(&txn))
        .await
        .internal()?;
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
    Ok(Some(Arc::new(OpenAiCompatibleProvider::new(config))))
}

/// What a workspace has configured, without the key itself.
///
/// `api_key` is absent rather than masked, as in `workspace_llm_keys::LlmKeyDescription`.
#[derive(Clone, Serialize, sea_orm::FromQueryResult, schemars::JsonSchema)]
#[schemars(rename = "EmbeddingKeyResponse")]
pub(crate) struct EmbeddingKeyDescription {
    pub(crate) query_instruction: String,
    pub(crate) document_prefix: String,
    pub(crate) base_url: String,
    pub(crate) model: String,
    pub(crate) dimensions: i32,
    /// Always true when present: the row cannot exist without a key.
    /// Callers use the absence of the whole description to mean "not configured".
    pub(crate) configured: bool,
}

fn dimension_count(dimensions: i32) -> Result<usize, YorishiroError> {
    if !(1..=4096).contains(&dimensions) {
        return Err(YorishiroError::ValidationFailed {
            message: "dimensions must be between 1 and 4096".into(),
            details: vec![],
            hint: "set it to the embedding model's own output width".into(),
        });
    }
    usize::try_from(dimensions).internal()
}

async fn save(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    config: OpenAiCompatibleConfig,
) -> Result<(), YorishiroError> {
    let active = ActiveModel {
        workspace_id: ActiveValue::Set(workspace_id),
        base_url: ActiveValue::Set(config.base_url),
        model: ActiveValue::Set(config.model),
        api_key: ActiveValue::Set(config.api_key),
        dimensions: ActiveValue::Set(i32::try_from(config.dimensions).internal()?),
        send_dimensions_param: ActiveValue::Set(config.send_dimensions_param),
        query_instruction: ActiveValue::Set(config.query_instruction),
        document_prefix: ActiveValue::Set(config.document_prefix),
        updated_at: ActiveValue::Set(crate::data::time::now().into()),
        ..Default::default()
    };
    let active = sea_orm::ActiveModelBehavior::before_save(active, conn, true)
        .await
        .internal()?;
    Entity::insert(active)
        .on_conflict(
            OnConflict::column(Column::WorkspaceId)
                .update_columns([
                    Column::BaseUrl,
                    Column::Model,
                    Column::ApiKey,
                    Column::Dimensions,
                    Column::SendDimensionsParam,
                    Column::QueryInstruction,
                    Column::DocumentPrefix,
                    Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(conn)
        .await
        .internal()?;
    touch_provider_revision(conn, workspace_id).await?;
    Ok(())
}

/// Marks the workspace's embedding provider as changed, which stales in-flight publication and requires a reindex.
async fn touch_provider_revision(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::workspace_workspaces::{
        Column as Workspace, Entity as Workspaces,
    };
    use sea_orm::ExprTrait;
    Workspaces::update_many()
        .col_expr(
            Workspace::EmbeddingRevision,
            sea_orm::sea_query::Expr::col(Workspace::EmbeddingRevision).add(1),
        )
        .col_expr(
            Workspace::EmbeddingReindexing,
            sea_orm::sea_query::Expr::value(true),
        )
        .filter(Workspace::Id.eq(workspace_id))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// Removes a workspace's own assignment.
/// It falls back to the deployment default afterward.
pub(crate) async fn clear(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    let result = Entity::delete_many()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .exec(conn)
        .await
        .internal()?;
    if result.rows_affected > 0 {
        touch_provider_revision(conn, workspace_id).await?;
    }
    Ok(())
}

pub(crate) async fn remove(
    ctx: &loco_rs::app::AppContext,
    actor: &AuthContext,
) -> Result<(), YorishiroError> {
    let workspace_id = actor.require_workspace()?;
    let txn = super::iam::access::workspace_transaction(
        &ctx.db,
        actor,
        Action::DeleteWorkspaceEmbeddingProvider,
    )
    .await?;
    clear(&txn, workspace_id).await?;
    crate::workers::reindex::enqueue_reindex(ctx, workspace_id, Some(&txn))
        .await
        .internal()?;
    txn.commit().await.internal()?;
    Ok(())
}

/// What is configured, for an endpoint to report.
/// Never includes the key.
pub(crate) async fn describe_as(
    db: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
) -> Result<Option<EmbeddingKeyDescription>, YorishiroError> {
    let txn =
        super::iam::access::workspace_transaction(db, actor, Action::GetWorkspaceEmbeddingProvider)
            .await?;
    describe(&txn, actor.require_workspace()?).await
}

pub(crate) async fn describe(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<EmbeddingKeyDescription>, YorishiroError> {
    Entity::find()
        .select_only()
        .columns([
            Column::BaseUrl,
            Column::Model,
            Column::Dimensions,
            Column::QueryInstruction,
            Column::DocumentPrefix,
        ])
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
    describe(conn, workspace_id)
        .await?
        .map(|description| {
            Ok(crate::services::embedding::EmbeddingMetadata {
                model: description.model,
                dimensions: usize::try_from(description.dimensions).internal()?,
                document_prefix: description.document_prefix,
            })
        })
        .transpose()
}

/// The credentials themselves, for building a provider.
/// `None` means the workspace has configured none, which the provider lookup reads as "fall back to the deployment default".
pub(crate) async fn get(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<OpenAiCompatibleConfig>, YorishiroError> {
    let row = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .one(conn)
        .await
        .internal()?;

    row.map(|row| {
        Ok(OpenAiCompatibleConfig {
            base_url: row.base_url,
            api_key: row.api_key,
            model: row.model,
            dimensions: usize::try_from(row.dimensions).internal()?,
            send_dimensions_param: row.send_dimensions_param,
            query_instruction: row.query_instruction,
            document_prefix: row.document_prefix,
        })
    })
    .transpose()
}
