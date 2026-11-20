//! Full reindex of a workspace: recomputes every vector with the current provider and publishes them atomically.

use sea_orm::{ConnectionTrait, DatabaseConnection, ExprTrait};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::entity_entities::EmbeddingSnapshot;
use crate::services::embedding::{EmbedKind, EmbeddingProvider};

use super::persistence::{VectorWriteInput, embed_and_write};

/// One entity that failed during a [`reindex_workspace`] run.
pub struct ReindexFailure {
    pub entity_id: Uuid,
    pub error: YorishiroError,
}

/// Outcome of a full [`reindex_workspace`] run.
#[derive(Default)]
pub struct ReindexOutcome {
    pub total: usize,
    pub reindexed: usize,
    pub failures: Vec<ReindexFailure>,
}

async fn reindex_embedding_for_record(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    record: &EmbeddingSnapshot,
    provider: &dyn EmbeddingProvider,
    schemas: &mut std::collections::HashMap<Uuid, crate::models::schema_schemas::SchemaRecord>,
) -> Result<Option<Vec<f32>>, YorishiroError> {
    // Schema definitions are versioned by ID; a new version cannot change this snapshot's schema.
    let schema = match schemas.entry(record.schema_id) {
        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::hash_map::Entry::Vacant(entry) => entry.insert(
            crate::models::schema_schemas::get_by_id(conn, workspace_id, record.schema_id).await?,
        ),
    };
    let entity_type_def = schema
        .definition
        .entity_types
        .get(&record.entity_type)
        .ok_or_else(|| {
            YorishiroError::not_found(format!(
                "entity_type '{}' is not defined in schema '{}'",
                record.entity_type, schema.definition.name
            ))
        })?;

    let Some(text) = super::write::compose_embedding_text(entity_type_def, &record.data) else {
        return Ok(None);
    };
    let vector = provider.embed_as(EmbedKind::Document, &text).await?;
    super::validate_vector(&vector, provider.dimensions())?;
    Ok(Some(vector))
}

/// Prepares every workspace vector without holding a transaction, then publishes all vectors and the stamp atomically.
/// Provider failures publish nothing and leave search gated until a successful retry.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn reindex_workspace(
    conn: &DatabaseConnection,
    workspace_id: Uuid,
    provider: &dyn EmbeddingProvider,
) -> Result<ReindexOutcome, YorishiroError> {
    reindex_workspace_as(conn, workspace_id, provider, None, false).await
}

async fn authorize_reindex(
    txn: &sea_orm::DatabaseTransaction,
    workspace_id: Uuid,
    actor: Option<&crate::models::api_keys::AuthContext>,
) -> Result<(), YorishiroError> {
    if let Some(actor) = actor {
        crate::models::iam::access::require_iam_action(
            txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Migration,
            crate::models::iam::policy::Action::WorkspaceReindex,
            crate::models::iam::policy::Resource::Workspace {
                tenant_id: actor.tenant_id,
                workspace_id,
            },
        )
        .await?;
    }
    Ok(())
}

async fn begin_reindex(
    conn: &DatabaseConnection,
    workspace_id: Uuid,
    provider: &dyn EmbeddingProvider,
    actor: Option<&crate::models::api_keys::AuthContext>,
    startup: bool,
) -> Result<Option<(i64, i64)>, YorishiroError> {
    use crate::models::_entities::workspace_workspaces::Column;
    use crate::models::workspace_workspaces;
    use sea_orm::sea_query::Expr;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let txn = crate::db::begin_write(conn).await.internal()?;
    authorize_reindex(&txn, workspace_id, actor).await?;
    let revision = workspace_workspaces::embedding_revision(&txn, workspace_id).await?;
    workspace_workspaces::fence_embedding(&txn, workspace_id, revision).await?;
    // A manual reindex may have published after the startup scan read its candidates.
    if startup
        && !workspace_workspaces::startup_reindex_needed(&txn, workspace_id, provider).await?
    {
        txn.rollback().await.internal()?;
        return Ok(None);
    }
    workspace_workspaces::check_embedding_assignment(&txn, workspace_id, provider).await?;
    let content_revision =
        workspace_workspaces::embedding_content_revision(&txn, workspace_id).await?;
    workspace_workspaces::Entity::update_many()
        .col_expr(
            Column::EmbeddingRevision,
            Expr::col(Column::EmbeddingRevision).add(1),
        )
        .filter(Column::Id.eq(workspace_id))
        .exec(&txn)
        .await
        .internal()?;
    workspace_workspaces::set_embedding_reindexing(&txn, workspace_id, true).await?;
    txn.commit().await.internal()?;
    Ok(Some((revision + 1, content_revision)))
}

async fn clear_workspace_vectors(
    txn: &sea_orm::DatabaseTransaction,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::{entity_embeddings as vectors, entity_entities as entities};
    use sea_orm::sea_query::{Expr, Query};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let entities = Query::select()
        .column(entities::Column::Id)
        .from(entities::Entity)
        .and_where(Expr::col(entities::Column::WorkspaceId).eq(workspace_id))
        .to_owned();
    super::Entity::delete_many()
        .filter(vectors::Column::WorkspaceId.eq(workspace_id))
        .filter(Expr::col(vectors::Column::EntityId).in_subquery(entities))
        .exec(txn)
        .await
        .internal()?;
    Ok(())
}

pub(crate) async fn reindex_workspace_as(
    conn: &DatabaseConnection,
    workspace_id: Uuid,
    provider: &dyn EmbeddingProvider,
    actor: Option<&crate::models::api_keys::AuthContext>,
    startup: bool,
) -> Result<ReindexOutcome, YorishiroError> {
    use crate::models::workspace_workspaces;

    if provider.dimensions() == 0 || i32::try_from(provider.dimensions()).is_err() {
        return Err(YorishiroError::ValidationFailed {
            message: "provider dimensions must be a positive i32".into(),
            details: vec![],
            hint: "configure the provider output width".into(),
        });
    }

    let Some((revision, content_revision)) =
        begin_reindex(conn, workspace_id, provider, actor, startup).await?
    else {
        return Ok(ReindexOutcome::default());
    };
    let candidate_ids = crate::models::entity_entities::ids_for_workspace(conn, workspace_id)
        .await
        .internal()?;
    let records =
        crate::models::entity_entities::get_batch_with_tokens(conn, workspace_id, &candidate_ids)
            .await
            .internal()?;

    let records: std::collections::HashMap<_, _> = records.into_iter().map(|r| (r.id, r)).collect();

    let mut reindexed = 0;
    let mut failures = Vec::new();
    // shortcut: prepared vectors use workspace-sized memory; stage them durably when large workspaces exceed the worker memory budget.
    let mut prepared = Vec::new();
    let mut schemas = std::collections::HashMap::new();
    for &entity_id in &candidate_ids {
        let Some(record) = records.get(&entity_id) else {
            failures.push(ReindexFailure {
                entity_id,
                error: YorishiroError::not_found(format!("entity {entity_id} no longer exists")),
            });
            continue;
        };
        match reindex_embedding_for_record(conn, workspace_id, record, provider, &mut schemas).await
        {
            Ok(vector) => prepared.push((record, vector)),
            Err(error) => failures.push(ReindexFailure { entity_id, error }),
        }
    }

    if failures.is_empty() {
        // Mutation triggers take this same row lock before changing entities or provider settings.
        let txn = crate::db::begin_write(conn).await.internal()?;
        authorize_reindex(&txn, workspace_id, actor).await?;
        workspace_workspaces::fence_embedding(&txn, workspace_id, revision).await?;
        if workspace_workspaces::embedding_content_revision(&txn, workspace_id).await?
            != content_revision
        {
            return Err(YorishiroError::Conflict {
                message: "workspace entities changed during reindex; retry the operation".into(),
            });
        }
        clear_workspace_vectors(&txn, workspace_id).await?;
        for (record, vector) in prepared {
            if let Some(vector) = vector {
                let written = embed_and_write(
                    &txn,
                    VectorWriteInput {
                        workspace_id,
                        entity_id: record.id,
                        embedding_sync_token: record.embedding_sync_token.clone(),
                        vector,
                    },
                )
                .await?;
                if !written {
                    return Err(YorishiroError::Conflict {
                        message: "entity disappeared during reindex publication".into(),
                    });
                }
            }
            reindexed += 1;
        }
        let dimensions = i32::try_from(provider.dimensions()).map_err(|_| {
            YorishiroError::Internal(
                format!(
                    "provider dimensions {} do not fit in an i32 column",
                    provider.dimensions()
                )
                .into(),
            )
        })?;
        super::stamp::restamp_workspace_embedding(
            &txn,
            workspace_id,
            provider.model_name(),
            dimensions,
            provider.document_prefix(),
        )
        .await?;
        txn.commit().await.internal()?;
    }

    Ok(ReindexOutcome {
        total: candidate_ids.len(),
        reindexed,
        failures,
    })
}
