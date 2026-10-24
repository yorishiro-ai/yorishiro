pub use crate::models::_entities::entity_snapshots::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::sqlite_generated_id(db, this.id);
        Ok(this)
    }
}
