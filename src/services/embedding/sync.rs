//! Generates and stores an entity's embedding vector after its own request transaction commits.
//! Ported from master's `services::embedding::sync`.
//!
//! Call this from a controller after `Authorized::commit()`, not before: it performs an HTTP
//! call to the embedding provider (up to 30s), and holding a DB connection or transaction for
//! that long risks connection pool exhaustion and lock contention. Runs on `ctx.db` (Loco's own
//! pooled `DatabaseConnection`), a fresh connection independent of the request's own transaction.

use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::EntityTypeDef;
use crate::models::content_entities::EntityRecord;
use crate::services::embedding::{EmbedKind, EmbeddingProvider};

/// Concatenates the values of `x-embed` fields as `"field: value"` to build the text to embed.
/// Field names are kept because bare values would lose semantic context that helps the embedding
/// model, compared to concatenating raw values alone.
/// Returns `None` when there are no such fields or all are absent, so callers can skip the
/// embedding API call entirely.
pub fn compose_embedding_text(entity_type_def: &EntityTypeDef, data: &Value) -> Option<String> {
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

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// Generates an embedding vector from an entity's `x-embed` fields and updates the
/// `content_entities.embedding` column. Returns `Ok(())` without doing anything if the schema
/// has no `x-embed` fields or none have values: embedding is an auxiliary feature and must never
/// block the entity write it follows.
pub async fn sync_embedding(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
    snapshot_updated_at: chrono::DateTime<chrono::Utc>,
    entity_type_def: &EntityTypeDef,
    data: &Value,
    provider: &dyn EmbeddingProvider,
) -> Result<(), YorishiroError> {
    let Some(text) = compose_embedding_text(entity_type_def, data) else {
        return Ok(());
    };

    let vector = provider.embed_as(EmbedKind::Document, &text).await?;

    // Refuse a vector that would not sit alongside what the workspace already holds. The column
    // is dimensionless, so a mismatched write succeeds and the damage surfaces somewhere else
    // entirely: the next search over that workspace fails with a dimension-mismatch error naming
    // neither the entity nor the write that caused it. Checking here turns a broken workspace
    // into one refused write.
    //
    // A workspace with no stamp (created before this existed) takes whatever the deployment
    // produces, which is what it has always done.
    if let Some(expected) = workspace_embedding_dimensions(conn, workspace_id).await?
        && vector.len() != expected as usize
    {
        return Err(YorishiroError::ValidationFailed {
            message: format!(
                "this workspace holds {expected}-dimensional vectors, but the configured \
                 embedding provider produced {}",
                vector.len()
            ),
            details: vec![],
            hint: "point the deployment at the workspace's model, or re-embed the workspace".into(),
        });
    }

    // Including the `updated_at` match as a write condition prevents a vector computed from
    // stale data from overwriting a newer one when consecutive updates to the same entity
    // complete out of order due to differing embedding API latencies (writing the embedding
    // itself doesn't change `updated_at`, so this condition never blocks a subsequent legitimate
    // sync).
    let result = conn
        .execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "UPDATE content_entities SET embedding = $1 \
             WHERE workspace_id = $2 AND id = $3 AND updated_at = $4",
            [
                pgvector::Vector::from(vector).into(),
                workspace_id.into(),
                entity_id.into(),
                snapshot_updated_at.into(),
            ],
        ))
        .await
        .internal()?;

    if result.rows_affected() == 0 {
        tracing::debug!(
            %entity_id,
            "sync_embedding: entity was deleted or updated since this snapshot, write skipped"
        );
    }

    Ok(())
}

/// Resolves the schema definition needed for embedding sync on its own, relying only on the
/// return value of `content_entities::create`/`update` (`EntityRecord`), then calls
/// [`sync_embedding`]. The record's data belongs to the schema version it was validated against
/// (`record.schema_id`), so fetching by ID rather than the active version is correct.
///
/// The intended entry point for controllers to call after `Authorized::commit()`.
pub async fn sync_embedding_for_record(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    record: &EntityRecord,
    provider: &dyn EmbeddingProvider,
) -> Result<(), YorishiroError> {
    let schema =
        crate::models::content_schemas::get_by_id(conn, workspace_id, record.schema_id).await?;
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

    sync_embedding(
        conn,
        workspace_id,
        record.id,
        record.updated_at,
        entity_type_def,
        &record.data,
        provider,
    )
    .await
}

/// The dimension count a workspace's vectors are expected to have, or `None` when it was created
/// before the stamp existed and therefore takes the deployment's.
async fn workspace_embedding_dimensions(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<i32>, YorishiroError> {
    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        embedding_dimensions: Option<i32>,
    }

    let row = Row::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT embedding_dimensions FROM identity_workspaces WHERE id = $1",
        [workspace_id.into()],
    ))
    .one(conn)
    .await
    .internal()?;
    Ok(row.and_then(|r| r.embedding_dimensions))
}
