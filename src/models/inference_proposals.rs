//! The `inference_proposals` table.
//! The overlay owns its behaviour, but every generated entity needs its own module so its `ActiveModelBehavior` is implemented in every build.

use super::_entities::inference_proposals::ActiveModel;

use sea_orm::entity::prelude::*;

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
