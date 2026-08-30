//! Generates and stores an entity's embedding vector after its own request transaction commits.
//!
//! Call this from a controller after `Authorized::commit()`, not before: it performs an HTTP call to the embedding provider (up to 30s), and holding a DB connection or transaction for that long risks connection pool exhaustion and lock contention.
//! Runs on `ctx.db` (Loco's own pooled `DatabaseConnection`), a fresh connection independent of the request's own transaction.

use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::EntityTypeDef;
use crate::models::content_entities::EntityRecord;
use crate::services::embedding::{EmbedKind, EmbeddingProvider};

/// Concatenates the values of `x-embed` fields as `"field: value"` to build the text to embed.
/// Field names are kept because bare values would lose semantic context that helps the embedding model, compared to concatenating raw values alone.
/// Returns `None` when there are no such fields or all are absent, so callers can skip the embedding API call entirely.
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

/// Generates an embedding vector from an entity's `x-embed` fields and updates the `content_entities.embedding` column.
/// Returns `Ok(())` without doing anything if the schema has no `x-embed` fields or none have values: embedding is an auxiliary feature and must never block the entity write it follows.
///
/// Checks the workspace's stamp before writing; see [`embed_and_write`] for the unguarded write this wraps, and why the reindex task calls that directly instead of this.
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

    // `content_entities.embedding` is `vector(768)` at the SQL type level, so a wrong-width write is already rejected by Postgres itself, not silently accepted: `pgvector` errors with "expected 768 dimensions, not N".
    // That error names neither the workspace nor the write that produced it, so this check exists to turn that into an operator-readable message (which workspace, which width was expected, what to do about it), not to prevent data corruption the database wasn't already preventing.
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

    // A second, different-natured check the dimension check above cannot subsume: two distinct
    // models can produce the same width (nomic-embed-text-v1.5 and multilingual-e5-base both output
    // 768, as of this writing) and coexist in the one `vector(768)` column above, since that column
    // only enforces width, not which model produced it. Postgres has no way to reject that mismatch
    // itself, unlike the width one above, which is why this check is the only thing standing between
    // a workspace and a silent write from the wrong model: it defends correctness, where the
    // dimension check above only improves an error message on a write Postgres would have refused
    // regardless.
    // The coexistence itself is a coincidence of both current models happening to share a width, not
    // something `content_entities` guarantees: the day a third local model ships at a different
    // width, the column's own fixed type already forces every workspace in this deployment onto one
    // width, and mixing stops being possible in the first place.
    if let Some(expected) = workspace_embedding_model(conn, workspace_id).await?
        && expected != "unconfigured"
        && expected != provider.model_name()
    {
        return Err(YorishiroError::ValidationFailed {
            message: format!(
                "this workspace was stamped with model {expected:?}, but the configured \
                 embedding provider is {:?}",
                provider.model_name()
            ),
            details: vec![],
            hint: "point the deployment at the workspace's stamped model, or run \
                   reindex_embeddings to switch it over"
                .into(),
        });
    }

    // A skipped write (entity changed since `snapshot_updated_at`) is not an error here: a newer
    // sync for the same entity is already in flight or has already landed, so nothing needs
    // redoing on this call's behalf. See `embed_and_write`'s own doc comment for why
    // `reindex_workspace` cannot make the same choice.
    embed_and_write(conn, workspace_id, entity_id, snapshot_updated_at, vector)
        .await
        .map(|_written| ())
}

/// Writes an already-computed embedding vector to `content_entities.embedding`, with no check against the workspace's stamped model or dimensions.
/// Returns whether the row was actually written: `false` means the `updated_at` guard below skipped the write because the entity changed since `snapshot_updated_at` was read, not an error.
///
/// [`sync_embedding`] wraps this with both checks for the normal write path, and ignores the returned bool: a skipped write there means a newer sync for the same entity is already in flight or has already landed, so nothing needs redoing.
/// [`reindex_workspace`] calls this directly instead: its entire job is changing which model a workspace's vectors were embedded with, so a check that refuses a write on exactly that mismatch would refuse its own writes on every row.
/// Safe to bypass here only because `reindex_workspace` restamps `identity_workspaces.embedding_model` itself, and only after every row succeeds: the stamp and the actual column contents genuinely disagree for its own duration, which is the situation `sync_embedding`'s check exists to prevent everywhere else.
/// `reindex_workspace` does *not* ignore the returned bool the way `sync_embedding` does: unlike an ordinary sync, a skipped reindex write means this entity's current data was never actually re-embedded with the new model, so counting it as reindexed would let the workspace restamp while that row still holds the old model's vector.
async fn embed_and_write(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
    snapshot_updated_at: chrono::DateTime<chrono::Utc>,
    vector: Vec<f32>,
) -> Result<bool, YorishiroError> {
    // `updated_at` as a write condition prevents a vector computed from stale data from overwriting a newer one when concurrent syncs for the same entity finish out of order.
    // Writing the embedding itself doesn't change `updated_at`, so this never blocks a later sync.
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
            "embed_and_write: entity was deleted or updated since this snapshot, write skipped"
        );
        return Ok(false);
    }

    Ok(true)
}

/// Resolves the schema definition needed for embedding sync on its own, relying only on the return value of `content_entities::create`/`update` (`EntityRecord`), then calls [`sync_embedding`].
/// The record's data belongs to the schema version it was validated against (`record.schema_id`), so fetching by ID rather than the active version is correct.
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

/// Whether [`reindex_embedding_for_record`] actually wrote a fresh vector for this entity.
#[derive(Debug, PartialEq, Eq)]
enum ReindexStep {
    /// The entity had `x-embed` fields and the write landed: this row's vector now reflects the new model.
    Reindexed,
    /// The entity's schema has no `x-embed` fields (or none had values), the same no-op [`sync_embedding`] would take: nothing to embed, and nothing wrong.
    NothingToEmbed,
    /// `embed_and_write` matched zero rows: the entity's data changed between this run's batch fetch and the write, so the vector just written is already stale for the entity's current data.
    /// Not the same as `NothingToEmbed`: this entity still has `x-embed` fields and still needs a real vector, just not the one this call produced.
    ConcurrentlyModified,
}

/// The reindex loop's per-entity step: composes the embedding text, embeds it, and writes it via [`embed_and_write`], bypassing both of [`sync_embedding`]'s checks (model stamp and dimension) for the reason documented on [`embed_and_write`] itself.
/// Otherwise identical to [`sync_embedding_for_record`]: same schema resolution, same no-op on an entity_type with no `x-embed` fields.
///
/// Skipping the dimension check specifically is harmless today only because `content_entities.embedding` is `vector(768)` at the SQL type level: Postgres itself still refuses a wrong-width write, `pgvector` erroring with "expected 768 dimensions, not N".
/// The day a differently-sized model is added and this deployment's column type changes to match, that raw Postgres error, naming neither the workspace nor the entity, becomes the first thing a reindex against the new model hits, rather than the readable message [`sync_embedding`]'s own dimension check would have given.
async fn reindex_embedding_for_record(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    record: &EntityRecord,
    provider: &dyn EmbeddingProvider,
) -> Result<ReindexStep, YorishiroError> {
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

    let Some(text) = compose_embedding_text(entity_type_def, &record.data) else {
        return Ok(ReindexStep::NothingToEmbed);
    };
    let vector = provider.embed_as(EmbedKind::Document, &text).await?;
    let written = embed_and_write(conn, workspace_id, record.id, record.updated_at, vector).await?;
    Ok(if written {
        ReindexStep::Reindexed
    } else {
        ReindexStep::ConcurrentlyModified
    })
}

/// One entity that failed during a [`reindex_workspace`] run, carrying enough to report it.
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

/// Re-embeds every entity in a workspace with `provider`, then restamps `identity_workspaces.embedding_model`/`embedding_dimensions` to that provider's own values, but only when every entity succeeded.
///
/// This is the core the `reindex_embeddings` task wraps; see that task's own doc comment for why a partial failure must leave the stamp untouched, and why bypassing [`sync_embedding`]'s model check here is safe only under that restamp-on-full-success ordering.
/// Callers needing every candidate id (rather than just those already `EntityRecord`-fetchable) pass them in as `candidate_ids`, since a row deleted between the caller's own scan and this function's batch fetch is reported as a failure rather than silently skipped.
pub async fn reindex_workspace(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    candidate_ids: &[Uuid],
    provider: &dyn EmbeddingProvider,
) -> Result<ReindexOutcome, YorishiroError> {
    let records = crate::models::content_entities::get_batch(conn, workspace_id, candidate_ids)
        .await
        .internal()?;

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
            // Not an error from the provider or the write itself: the entity changed between this
            // run's batch fetch and the write landing, so the vector just computed is already
            // stale. Counting this as reindexed would let the workspace restamp while this row
            // still holds the old model's vector; a plain re-run picks it up against its current
            // data instead.
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
        conn.execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "UPDATE identity_workspaces SET embedding_model = $1, embedding_dimensions = $2 \
             WHERE id = $3",
            [
                provider.model_name().into(),
                dimensions.into(),
                workspace_id.into(),
            ],
        ))
        .await
        .internal()?;
    }

    Ok(ReindexOutcome {
        total: candidate_ids.len(),
        reindexed,
        failures,
    })
}

/// The dimension count a workspace's vectors are expected to have, or `None` for a workspace carrying no stamp of its own, which takes the deployment's.
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

/// The model a workspace's vectors are expected to have been embedded with, or `None` for a workspace carrying no stamp of its own.
///
/// A `Some("unconfigured")` row is not the same as `None` and the caller must not treat them alike: `None` means this workspace predates the `embedding_model` column entirely or was never stamped, while `"unconfigured"` means a workspace created with no embedding provider available at all ([`super::UnconfiguredEmbeddingProvider`] stamps exactly this string).
/// Both are honestly "no real record of what this workspace was embedded with", which is why the caller skips the comparison for `"unconfigured"` too rather than trying to distinguish it from `None` here.
async fn workspace_embedding_model(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<String>, YorishiroError> {
    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        embedding_model: Option<String>,
    }

    let row = Row::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT embedding_model FROM identity_workspaces WHERE id = $1",
        [workspace_id.into()],
    ))
    .one(conn)
    .await
    .internal()?;
    Ok(row.and_then(|r| r.embedding_model))
}
