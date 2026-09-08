//! Width-partitioned embedding storage.
//!
//! Embeddings are stored in separate tables per dimension:
//! `content_entity_embeddings_768`, `content_entity_embeddings_1024`,
//! `content_entity_embeddings_1536`. Each re-exports its Entity, Model,
//! and ActiveModel from the generated entity.
//!
//! The `ContentEntityEmbeddings768`, `ContentEntityEmbeddings1024`,
//! and `ContentEntityEmbeddings1536` type aliases allow callers to refer
//! to a specific width without importing the `_entities` module directly.

pub use super::_entities::content_entity_embeddings_768::{
    ActiveModel as ActiveModel768, Entity as Entity768, Model as Model768,
};
pub use super::_entities::content_entity_embeddings_1024::{
    ActiveModel as ActiveModel1024, Entity as Entity1024, Model as Model1024,
};
pub use super::_entities::content_entity_embeddings_1536::{
    ActiveModel as ActiveModel1536, Entity as Entity1536, Model as Model1536,
};

/// Type alias for the 768-width entity.
pub type ContentEntityEmbeddings768 = Entity768;
/// Type alias for the 1024-width entity.
pub type ContentEntityEmbeddings1024 = Entity1024;
/// Type alias for the 1536-width entity.
pub type ContentEntityEmbeddings1536 = Entity1536;
