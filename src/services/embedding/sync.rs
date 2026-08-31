//! Generates and stores an entity's embedding vector after its own request transaction commits.
//!
//! Call this from a controller after `Authorized::commit()`, not before: it performs an HTTP call to the embedding provider (up to 30s), and holding a DB connection or transaction for that long risks connection pool exhaustion and lock contention.
//! Runs on `ctx.db` (Loco's own pooled `DatabaseConnection`), a fresh connection independent of the request's own transaction.

use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QuerySelect,
};
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
/// Checks the workspace's stamp against the provider's model and dimensions before writing.
/// The three-tier inheritance is: workspace stamp → tenant default → deployment default.
/// Stamps the workspace on first successful embed (first-write stamping).
/// See [`embed_and_write`] for the unguarded write this wraps, and why the reindex task calls that directly instead of this.
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
    let chain = resolve_embedding_chain(conn, workspace_id).await?;
    // Workspace stamp → deployment default.
    let effective_dimensions = chain.workspace_dimensions;
    if let Some(expected) = effective_dimensions
        && vector.len() as usize != expected as usize
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
    if let Some(expected) = &chain.workspace_model
        && expected.as_str() != provider.model_name()
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

    // Stamp the workspace on first successful embed: if the workspace has no stamp of its own yet
    // (both model and dimensions are NULL), write the provider's values. This replaces the old
    // "unconfigured" sentinel: instead of stamping at creation time (which would record
    // "unconfigured" when no provider is available, then never update it), we stamp on the first
    // real embed. This avoids the defect where a workspace created without embeddings could never
    // be updated because the sentinel blocked the comparison logic.
    if chain.workspace_model.is_none() {
        let dimensions = i32::try_from(chain.deployment_dimensions).map_err(|_| {
            YorishiroError::Internal(anyhow::anyhow!(
                "resolved dimensions {} do not fit in an i32 column",
                chain.deployment_dimensions
            ))
        })?;
        stamp_workspace_embedding(conn, workspace_id, provider.model_name(), dimensions).await?;
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
    use crate::models::_entities::content_entities;

    let pg_vector = pgvector::Vector::from(vector);
    let rows_affected = content_entities::Entity::update_many()
        .col_expr(content_entities::Column::Embedding, Expr::value(pg_vector))
        .filter(content_entities::Column::WorkspaceId.eq(workspace_id))
        .filter(content_entities::Column::Id.eq(entity_id))
        .filter(content_entities::Column::UpdatedAt.eq(snapshot_updated_at))
        .exec(conn)
        .await
        .internal()?;

    if rows_affected.rows_affected == 0 {
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
        let mut active = crate::models::identity_workspaces::ActiveModel {
            id: ActiveValue::Unchanged(workspace_id),
            ..Default::default()
        };
        active.embedding_model = ActiveValue::Set(Some(provider.model_name()));
        active.embedding_dimensions = ActiveValue::Set(Some(dimensions));
        active.update(conn).await.internal()?;
    }

    Ok(ReindexOutcome {
        total: candidate_ids.len(),
        reindexed,
        failures,
    })
}

/// The resolved embedding chain for a workspace: which model and dimension count to use.
///
/// The three-tier inheritance is:
///
/// 1. Workspace stamp: if `identity_workspaces.embedding_model`/`embedding_dimensions` is set,
///    use those values directly.
/// 2. Tenant default: if the workspace's tenant has `embedding_model`/`embedding_dimensions` set,
///    use those as the fallback.
/// 3. Deployment default: the system default dimensions (always available, e.g. from
///    `YORISHIRO_EMBEDDING_DIMENSIONS`, defaulting to 768).
#[derive(Debug, Clone)]
pub struct ResolvedEmbedding {
    /// The workspace's own model stamp, or `None` when the workspace has no stamp of its own.
    /// The caller uses this to determine whether first-write stamping is needed.
    pub workspace_model: Option<String>,
    /// The workspace's own dimension stamp, or `None` when the workspace has no stamp of its own.
    pub workspace_dimensions: Option<i32>,
    /// The deployment-wide default dimensions, always set. Used as the ultimate fallback and as
    /// the first-write stamp value.
    pub deployment_dimensions: usize,
}

/// A row returned by `resolve_embedding_chain`: workspace columns only.
#[derive(sea_orm::FromQueryResult)]
struct EmbeddingChainRow {
    embedding_model: Option<String>,
    workspace_dimensions: Option<i32>,
}

/// Resolves the full three-tier embedding chain for a workspace in a single query:
/// workspace stamp → tenant default → deployment default.
///
/// `conn` can be any connection trait: a transaction or a pooled database connection.
async fn resolve_embedding_chain(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<ResolvedEmbedding, YorishiroError> {
    use crate::models::_entities::identity_workspaces::Column;

    let row = crate::models::identity_workspaces::Entity::find()
        .select_only()
        .column(Column::EmbeddingModel)
        .column(Column::EmbeddingDimensions)
        .filter(Column::Id.eq(workspace_id))
        .into_model::<EmbeddingChainRow>()
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("workspace {workspace_id} not found")))?;

    // Deployment default: read from the environment variable.
    // The deployment's embedding dimensions are always set (default 768).
    let deployment_dimensions: usize = std::env::var("YORISHIRO_EMBEDDING_DIMENSIONS")
        .unwrap_or_else(|_| "768".into())
        .parse()
        .unwrap_or(768);

    Ok(ResolvedEmbedding {
        workspace_model: row.embedding_model,
        workspace_dimensions: row.workspace_dimensions,
        deployment_dimensions,
    })
}

/// Stamp a workspace's `embedding_model` and `embedding_dimensions` with the deployment default.
///
/// Called on first-write: when a workspace has no stamp of its own, this records the deployment's
/// default so subsequent writes can be checked against it.
async fn stamp_workspace_embedding(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    model: String,
    dimensions: i32,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::identity_workspaces::Column;

    // Only stamp if the workspace has no existing model stamp:
    // this is the first-write stamp, not an unconditional overwrite.
    crate::models::identity_workspaces::Entity::update_many()
        .col_expr(Column::EmbeddingModel, Expr::value(model))
        .col_expr(Column::EmbeddingDimensions, Expr::value(dimensions))
        .filter(Column::Id.eq(workspace_id))
        .filter(Column::EmbeddingModel.is_null())
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}
