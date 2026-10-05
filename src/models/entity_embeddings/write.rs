use sea_orm::{ConnectionTrait, DbErr};
use serde_json::Value;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::entity_entities;
use crate::models::entity_entities::EmbeddingSnapshot;
use crate::models::entity_entities::EntityRecord;
use crate::models::schema_schemas::metaschema::EntityTypeDef;
use crate::services::embedding::{EmbedKind, EmbeddingProvider};

use super::persistence::{VectorWriteInput, embed_and_write};

/// Concatenates the values of `x-embed` fields as `"field: value"` to build the text to embed.
pub(super) fn compose_embedding_text(
    entity_type_def: &EntityTypeDef,
    data: &Value,
) -> Option<String> {
    let parts: Vec<String> = entity_type_def
        .fields
        .iter()
        .filter(|(_, field_def)| field_def.x_embed)
        .filter_map(|(name, _)| match data.get(name) {
            Some(Value::String(s)) => Some(format!("{name}: {s}")),
            Some(Value::Null) | None => None,
            Some(other) => Some(format!("{name}: {other}")),
        })
        .collect();

    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// Generates and stores one entity's embedding, checking it against what the workspace already holds.
async fn sync_embedding(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    record: &EmbeddingSnapshot,
    entity_type_def: &EntityTypeDef,
    provider: &dyn EmbeddingProvider,
) -> Result<(), YorishiroError> {
    let Some(text) = compose_embedding_text(entity_type_def, &record.data) else {
        return Ok(());
    };

    let vector = provider.embed_as(EmbedKind::Document, &text).await?;
    let chain =
        super::resolution::resolve_embedding_chain(conn, workspace_id, provider.dimensions())
            .await?;
    let expected_dimensions = chain
        .workspace_dimensions
        .or(chain.tenant_dimensions)
        .unwrap_or(
            i32::try_from(chain.deployment_dimensions)
                .unwrap_or(crate::services::embedding::DEFAULT_EMBEDDING_DIMENSIONS as i32),
        );
    if vector.len() != expected_dimensions as usize {
        return Err(YorishiroError::ValidationFailed {
            message: format!(
                "this workspace holds {expected_dimensions}-dimensional vectors, but the configured embedding \
                 provider produced {}",
                vector.len()
            ),
            details: vec![],
            hint: "point the deployment at the workspace's model, or re-embed the workspace".into(),
        });
    }

    let effective_model = chain
        .workspace_model
        .as_ref()
        .or(chain.tenant_model.as_ref());
    if let Some(expected) = effective_model
        && expected.as_str() != provider.model_name()
    {
        return Err(YorishiroError::ValidationFailed {
            message: format!(
                "this workspace expects model {expected:?}, but the configured embedding \
                 provider is {:?}",
                provider.model_name()
            ),
            details: vec![],
            hint: "point the deployment at the workspace's effective model, or run \
                   reindex_embeddings to switch it over"
                .into(),
        });
    }

    let written = embed_and_write(
        conn,
        VectorWriteInput {
            workspace_id,
            entity_id: record.id,
            embedding_sync_token: record.embedding_sync_token.clone(),
            vector,
            dimension: expected_dimensions as usize,
        },
    )
    .await?;

    if chain.workspace_model.is_none() && written {
        let dimensions = i32::try_from(provider.dimensions()).map_err(|_| {
            YorishiroError::Internal(anyhow::anyhow!(
                "provider dimensions {} do not fit in an i32 column",
                provider.dimensions()
            ))
        })?;
        super::stamp::stamp_workspace_embedding(
            conn,
            workspace_id,
            provider.model_name(),
            dimensions,
        )
        .await?;
    }

    Ok(())
}

/// Resolves the schema definition for an entity record and synchronizes its embedding.
///
/// Accepts an `EntityRecord` (the original public type) so the public API shape is preserved
/// and the concurrency token never leaks into `EntityRecord`.  Behind the scenes this re-reads
/// the current `EmbeddingSnapshot` to pick up the latest concurrency token, then delegates to
/// the private `sync_embedding_for_snapshot`.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn sync_embedding_for_record(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    record: &EntityRecord,
    provider: &dyn EmbeddingProvider,
) -> Result<(), YorishiroError> {
    // Re-read the full snapshot with the current token: the caller has an EntityRecord
    // without token info, so we need the latest token from the DB to guard embed_and_write.
    let snapshot = entity_entities::get_with_token(conn, workspace_id, record.id)
        .await
        .map_err(|error| match error {
            DbErr::RecordNotFound(_) => {
                YorishiroError::not_found(format!("entity '{}' was not found", record.id))
            }
            other => YorishiroError::Internal(other.into()),
        })?;

    sync_embedding_for_snapshot(conn, workspace_id, &snapshot, provider).await
}

/// Resolves the schema definition for an entity snapshot and synchronizes its embedding.
///
/// This is the private entry point used by the embedding sync worker and `resync_embeddings`
/// task after their own atomic snapshot read (which already includes the concurrency token).
/// The public `sync_embedding_for_record` wraps this after re-reading the snapshot.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub(crate) async fn sync_embedding_for_snapshot(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    record: &EmbeddingSnapshot,
    provider: &dyn EmbeddingProvider,
) -> Result<(), YorishiroError> {
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

    sync_embedding(conn, workspace_id, record, entity_type_def, provider).await
}
