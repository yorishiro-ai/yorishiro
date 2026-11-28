//! The `stripe_events` table.
//! Only the overlay billing code reads and writes it, but every generated entity needs its own module so its `ActiveModelBehavior` is implemented in every build.

pub use crate::models::_entities::stripe_events::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.created_at = crate::db::stamped_created_at(insert, this.created_at);
        Ok(this)
    }
}
