//! Variable-dimension embedding storage and workspace reindex operations.

mod persistence;
mod reindex;
mod resolution;
mod stamp;
mod write;

pub use reindex::{ReindexFailure, ReindexOutcome, reindex_workspace};
pub(crate) use write::EmbeddingWriteOutcome;
pub use write::sync_embedding_for_record;
pub(crate) use write::sync_embedding_for_snapshot;

pub(crate) const TABLE: &str = "entity_embeddings";

pub use crate::models::_entities::entity_embeddings::{ActiveModel, Entity, Model};

impl sea_orm::ActiveModelBehavior for ActiveModel {}

pub(super) fn validate_vector(
    vector: &[f32],
    expected_dimensions: usize,
) -> Result<(), crate::YorishiroError> {
    if vector.is_empty()
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
