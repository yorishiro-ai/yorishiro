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

pub use crate::models::_entities::entity_embeddings_768::{
    ActiveModel as ActiveModel768, Entity as Entity768, Model as Model768,
};
pub use crate::models::_entities::entity_embeddings_1024::{
    ActiveModel as ActiveModel1024, Entity as Entity1024, Model as Model1024,
};
pub use crate::models::_entities::entity_embeddings_1536::{
    ActiveModel as ActiveModel1536, Entity as Entity1536, Model as Model1536,
};
