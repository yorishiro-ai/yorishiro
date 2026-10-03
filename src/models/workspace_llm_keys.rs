pub use crate::models::_entities::workspace_llm_keys::{ActiveModel, Entity, Model};

use std::fmt;

impl fmt::Debug for Model {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceLlmKey")
            .field("workspace_id", &self.workspace_id)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}
use sea_orm::entity::prelude::*;

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
