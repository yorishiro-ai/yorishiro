//! The `workspace_schema_forks` table.
//! Only the overlay schema forks write it, but the base deletes and inspects its rows when a tenant is retired, so every build needs its `ActiveModelBehavior`.

pub use crate::models::_entities::workspace_schema_forks::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::sqlite_generated_id(db, this.id);
        this.updated_at = crate::db::stamped_updated_at(insert, this.updated_at);
        Ok(this)
    }
}
