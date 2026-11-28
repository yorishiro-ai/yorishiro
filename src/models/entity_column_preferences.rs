//! The `entity_column_preferences` table, which holds per-workspace column choices for the Entities table.
//!
//! Only the overlay reads and writes it, but every generated entity needs its own module so its `ActiveModelBehavior` is implemented in every build.

pub use crate::models::_entities::entity_column_preferences::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::generated_id(this.id);
        this.created_at = crate::db::stamped_created_at(insert, this.created_at);
        this.updated_at = crate::db::stamped_updated_at(this.updated_at);
        Ok(this)
    }
}
