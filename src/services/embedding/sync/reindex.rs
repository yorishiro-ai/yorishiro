use sea_orm::ConnectionTrait;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::content::entity_entities::EntityRecord;
use crate::services::embedding::{EmbedKind, EmbeddingProvider};

use super::persistence::{VectorWriteInput, embed_and_write};
use super::{ReindexFailure, ReindexOutcome};

#[derive(PartialEq, Eq)]
enum ReindexStep {
    Reindexed,
    NothingToEmbed,
    ConcurrentlyModified,
}

async fn reindex_embedding_for_record(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    record: &EntityRecord,
    provider: &dyn EmbeddingProvider,
) -> Result<ReindexStep, YorishiroError> {
    let schema =
        crate::models::content::schema_schemas::get_by_id(conn, workspace_id, record.schema_id)
            .await?;
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
    let written = embed_and_write(
        conn,
        VectorWriteInput {
            workspace_id,
            entity_id: record.id,
            snapshot_updated_at: record.updated_at,
            vector,
            dimension: provider.dimensions(),
        },
    )
    .await?;
    Ok(if written {
        ReindexStep::Reindexed
    } else {
        ReindexStep::ConcurrentlyModified
    })
}

pub(super) async fn run(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    candidate_ids: &[Uuid],
    provider: &dyn EmbeddingProvider,
) -> Result<ReindexOutcome, YorishiroError> {
    let records =
        crate::models::content::entity_entities::get_batch(conn, workspace_id, candidate_ids)
            .await
            .map_err(|err| YorishiroError::Internal(err.into()))?;

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
            Ok(ReindexStep::ConcurrentlyModified) => failures.push(ReindexFailure {
                entity_id,
                error: YorishiroError::Internal(anyhow::anyhow!(
                    "entity was modified concurrently with the reindex; re-run reindex_embeddings \
                     to pick it up against its current data"
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
