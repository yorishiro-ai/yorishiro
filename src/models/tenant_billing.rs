//! The `tenant_billing` table.
//! Only the overlay billing code reads and writes it, but every generated entity needs its own module so its `ActiveModelBehavior` is implemented in every build.

use sea_orm::entity::prelude::*;

pub use crate::models::_entities::tenant_billing::{ActiveModel, Entity, Model};

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.updated_at = crate::db::stamped_updated_at(insert, this.updated_at);
        Ok(this)
    }
}
