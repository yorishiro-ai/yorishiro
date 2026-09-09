//! Width-partitioned embedding storage.
//!
//! Embeddings are stored in separate tables per dimension:
//! `entity_embeddings_768`, `entity_embeddings_1024`,
//! `entity_embeddings_1536`. Each re-exports its Entity, Model,
//! and ActiveModel from the generated entity.

pub use super::_entities::entity_embeddings_768::{
    ActiveModel as ActiveModel768, Entity as Entity768, Model as Model768,
};
pub use super::_entities::entity_embeddings_1024::{
    ActiveModel as ActiveModel1024, Entity as Entity1024, Model as Model1024,
};
pub use super::_entities::entity_embeddings_1536::{
    ActiveModel as ActiveModel1536, Entity as Entity1536, Model as Model1536,
};
