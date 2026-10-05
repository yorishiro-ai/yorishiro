use sea_orm::ConnectionTrait;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::entity_entities::EmbeddingSnapshot;
use crate::services::embedding::{EmbedKind, EmbeddingProvider};

use super::persistence::{VectorWriteInput, embed_and_write};

#[derive(PartialEq, Eq)]
enum ReindexStep {
    Reindexed,
    NothingToEmbed,
    Deleted,
}

/// One entity that failed during a [`reindex_workspace`] run.
pub struct ReindexFailure {
    pub entity_id: Uuid,
    pub error: YorishiroError,
}

/// Outcome of a full [`reindex_workspace`] run.
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
) -> Result<ReindexStep, YorishiroError> {
    let schema =
        crate::models::schema_schemas::get_by_id(conn, workspace_id, record.schema_id).await?;
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
        return Ok(ReindexStep::NothingToEmbed);
    };
    let vector = provider.embed_as(EmbedKind::Document, &text).await?;
    match embed_and_write(
        conn,
        VectorWriteInput {
            workspace_id,
            entity_id: record.id,
            embedding_sync_token: record.embedding_sync_token.clone(),
            vector,
            dimension: provider.dimensions(),
        },
    )
    .await?
    {
        true => Ok(ReindexStep::Reindexed),
        false => Ok(ReindexStep::Deleted),
    }
}

/// Re-embeds every candidate entity and restamps the workspace after full success.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn reindex_workspace(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    candidate_ids: &[Uuid],
    provider: &dyn EmbeddingProvider,
) -> Result<ReindexOutcome, YorishiroError> {
    let records =
        crate::models::entity_entities::get_batch_with_tokens(conn, workspace_id, candidate_ids)
            .await
            .internal()?;

    let records: std::collections::HashMap<_, _> = records.into_iter().map(|r| (r.id, r)).collect();

    let mut reindexed = 0;
    let mut failures = Vec::new();
    for &entity_id in candidate_ids {
        let Some(record) = records.get(&entity_id) else {
            failures.push(ReindexFailure {
                entity_id,
                error: YorishiroError::not_found(format!("entity {entity_id} no longer exists")),
            });
            continue;
        };
        match reindex_embedding_for_record(conn, workspace_id, record, provider).await {
            Ok(ReindexStep::Reindexed | ReindexStep::NothingToEmbed) => reindexed += 1,
            Ok(ReindexStep::Deleted) => failures.push(ReindexFailure {
                entity_id,
                error: YorishiroError::not_found(format!(
                    "entity {entity_id} was deleted during the reindex"
                )),
            }),
            Err(error) => failures.push(ReindexFailure { entity_id, error }),
        }
    }

    if failures.is_empty() {
        let dimensions = i32::try_from(provider.dimensions()).map_err(|_| {
            YorishiroError::Internal(anyhow::anyhow!(
                "provider dimensions {} do not fit in an i32 column",
                provider.dimensions()
            ))
        })?;
        super::stamp::restamp_workspace_embedding(
            conn,
            workspace_id,
            provider.model_name(),
            dimensions,
        )
        .await?;
    }

    Ok(ReindexOutcome {
        total: candidate_ids.len(),
        reindexed,
        failures,
    })
}
