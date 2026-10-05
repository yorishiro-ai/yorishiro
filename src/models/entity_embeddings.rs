//! Width-partitioned embedding storage, and the operations that write to it.
//!
//! Embeddings are stored in separate tables per dimension:
//! `entity_embeddings_768`, `entity_embeddings_1024`,
//! `entity_embeddings_1536`. Each re-exports its Entity, Model,
//! and ActiveModel from the generated entity.

mod persistence;
mod reindex;
mod resolution;
mod stamp;
mod write;

pub use reindex::{ReindexFailure, ReindexOutcome, reindex_workspace};
pub(crate) use resolution::resolve_embedding_chain;
pub use write::sync_embedding_for_record;
pub(crate) use write::sync_embedding_for_snapshot;

/// The embedding widths with a table, in the order the migration creates them.
/// Every raw SQL statement that names an embedding table takes it from here.
const EMBEDDING_TABLES: [(usize, &str); 3] = [
    (768, "entity_embeddings_768"),
    (1024, "entity_embeddings_1024"),
    (1536, "entity_embeddings_1536"),
];

/// The table holding `dimension`-wide vectors.
///
/// # Errors
/// Returns `ValidationFailed` when no table exists for that width.
pub(crate) fn embedding_table(dimension: usize) -> Result<&'static str, crate::YorishiroError> {
    EMBEDDING_TABLES
        .iter()
        .find(|(width, _)| *width == dimension)
        .map(|(_, table)| *table)
        .ok_or_else(|| crate::YorishiroError::ValidationFailed {
            message: format!("no embedding table exists for {dimension}-dimensional vectors"),
            details: vec![],
            hint: format!(
                "supported dimensions: {}",
                EMBEDDING_TABLES
                    .iter()
                    .map(|(width, _)| width.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })
}

/// Every embedding table name.
pub(crate) fn embedding_tables() -> impl Iterator<Item = &'static str> {
    EMBEDDING_TABLES.iter().map(|(_, table)| *table)
}

pub use crate::models::_entities::entity_embeddings_768::{
    ActiveModel as ActiveModel768, Entity as Entity768, Model as Model768,
};
pub use crate::models::_entities::entity_embeddings_1024::{
    ActiveModel as ActiveModel1024, Entity as Entity1024, Model as Model1024,
};
pub use crate::models::_entities::entity_embeddings_1536::{
    ActiveModel as ActiveModel1536, Entity as Entity1536, Model as Model1536,
};
