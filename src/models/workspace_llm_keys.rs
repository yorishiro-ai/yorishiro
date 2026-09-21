pub use super::_entities::workspace_llm_keys::{ActiveModel, Entity};
use sea_orm::entity::prelude::*;

/// A safe read record for workspace LLM configuration.
/// The generated entity model is kept behind the entity module because it contains the raw API key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub workspace_id: Uuid,
    pub base_url: String,
    pub model: String,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.updated_at = crate::db::stamped_updated_at(insert, this.updated_at);
        Ok(this)
    }
}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}
