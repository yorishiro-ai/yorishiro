use sea_orm::entity::prelude::*;
pub use super::_entities::entity_embeddings_768::{ActiveModel, Entity, Model};
pub type EntityEmbeddings768 = Entity;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(
        self,
        _db: &C,
        _insert: bool,
    ) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        Ok(self)
    }
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selections oriented logic here
impl Entity {}
