use sea_orm::ConnectionTrait;
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
) -> Result<bool, YorishiroError> {
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
        return Ok(true);
    };
    let vector = provider.embed_as(EmbedKind::Document, &text).await?;
    super::validate_vector(&vector, provider.dimensions())?;
    embed_and_write(
        conn,
        VectorWriteInput {
            workspace_id,
            entity_id: record.id,
            embedding_sync_token: record.embedding_sync_token.clone(),
            vector,
        },
    )
    .await
}

/// Re-embeds every entity in the workspace and restamps it after full success.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn reindex_workspace(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    provider: &dyn EmbeddingProvider,
) -> Result<ReindexOutcome, YorishiroError> {
    crate::models::workspace_workspaces::set_embedding_reindexing(conn, workspace_id, true).await?;
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
            Ok(true) => reindexed += 1,
            Ok(false) => failures.push(ReindexFailure {
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
