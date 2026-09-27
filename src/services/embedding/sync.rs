//! Stable facade for embedding synchronization and workspace reindexing.

mod persistence;
mod reindex;
mod resolution;
mod stamp;
mod write;

use sea_orm::ConnectionTrait;
use serde_json::Value;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::metaschema::EntityTypeDef;
use crate::models::entity_entities::EntityRecord;
use crate::services::embedding::EmbeddingProvider;

/// Concatenates the values of `x-embed` fields as `"field: value"` to build the text to embed.
pub fn compose_embedding_text(entity_type_def: &EntityTypeDef, data: &Value) -> Option<String> {
    write::compose_embedding_text(entity_type_def, data)
}

/// Generates and stores an entity embedding after the entity write commits.
#[allow(clippy::too_many_arguments)]
pub async fn sync_embedding(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
    snapshot_updated_at: chrono::DateTime<chrono::Utc>,
    entity_type_def: &EntityTypeDef,
    data: &Value,
    provider: &dyn EmbeddingProvider,
    licenced: bool,
) -> Result<(), YorishiroError> {
    write::sync_embedding(
        conn,
        workspace_id,
        entity_id,
        snapshot_updated_at,
        entity_type_def,
        data,
        provider,
        licenced,
    )
    .await
}

/// Resolves the schema definition for an entity record and synchronizes its embedding.
pub async fn sync_embedding_for_record(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    record: &EntityRecord,
    provider: &dyn EmbeddingProvider,
    licenced: bool,
) -> Result<(), YorishiroError> {
    write::sync_embedding_for_record(conn, workspace_id, record, provider, licenced).await
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

/// Re-embeds every candidate entity and restamps the workspace after full success.
pub async fn reindex_workspace(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    candidate_ids: &[Uuid],
    provider: &dyn EmbeddingProvider,
) -> Result<ReindexOutcome, YorishiroError> {
    reindex::run(conn, workspace_id, candidate_ids, provider).await
}

/// The resolved embedding chain for a workspace.
#[derive(Clone)]
pub(crate) struct ResolvedEmbedding {
    pub(crate) workspace_model: Option<String>,
    pub(crate) workspace_dimensions: Option<i32>,
    pub(crate) tenant_model: Option<String>,
    pub(crate) tenant_dimensions: Option<i32>,
    pub(crate) deployment_dimensions: usize,
}

/// A row returned by the workspace and tenant embedding-chain query.
#[derive(sea_orm::FromQueryResult)]
struct EmbeddingChainRow {
    embedding_model: Option<String>,
    embedding_dimensions: Option<i32>,
    tenant_model: Option<String>,
    tenant_dimensions: Option<i32>,
}

/// A workspace row used by startup reindex detection.
#[derive(Clone, sea_orm::FromQueryResult)]
#[allow(dead_code)]
pub(crate) struct StartupReindexRow {
    pub id: Uuid,
    pub embedding_model: Option<String>,
    pub embedding_dimensions: Option<i32>,
    pub tenant_model: Option<String>,
    pub tenant_dimensions: Option<i32>,
}

/// Resolves the workspace, tenant, and deployment embedding settings.
pub(crate) async fn resolve_embedding_chain(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    licenced: bool,
) -> Result<ResolvedEmbedding, YorishiroError> {
    resolution::resolve_embedding_chain(conn, workspace_id, licenced).await
}
