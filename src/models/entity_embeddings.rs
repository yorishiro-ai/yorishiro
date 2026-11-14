//! Variable-dimension embedding storage and workspace reindex operations.

mod indexes;
mod persistence;
mod reindex;
mod resolution;
mod stamp;
mod write;

pub use indexes::maintain_workspace_indexes;
pub use reindex::{ReindexFailure, ReindexOutcome, reindex_workspace};
pub(crate) use write::EmbeddingWriteOutcome;
pub use write::sync_embedding_for_record;
pub(crate) use write::sync_embedding_for_snapshot;

pub(crate) const TABLE: &str = "entity_embeddings";

/// Largest embedding width the application stores or searches.
pub(crate) const MAX_EMBEDDING_DIMENSIONS: usize = 4096;
/// pgvector's dense HNSW limit for the full-precision `vector` type.
pub(crate) const PGVECTOR_HNSW_VECTOR_MAX_DIMS: usize = 2000;
/// pgvector's dense HNSW limit for the half-precision `halfvec` type.
pub(crate) const PGVECTOR_HNSW_HALFVEC_MAX_DIMS: usize = 4000;

pub use crate::models::_entities::entity_embeddings::{ActiveModel, Entity, Model};

impl sea_orm::ActiveModelBehavior for ActiveModel {}

pub(super) fn validate_vector(
    vector: &[f32],
    expected_dimensions: usize,
) -> Result<(), crate::YorishiroError> {
    if vector.is_empty()
        || vector.len() > MAX_EMBEDDING_DIMENSIONS
        || vector.len() != expected_dimensions
        || vector.iter().any(|value| !value.is_finite())
    {
        return Err(crate::YorishiroError::ValidationFailed {
            message: format!(
                "embedding vector must contain {expected_dimensions} finite values, got {} values",
                vector.len()
            ),
            details: vec![],
            hint: "configure the provider to return its declared embedding dimensions".into(),
        });
    }
    Ok(())
}

pub(crate) use reindex::reindex_workspace_as;
