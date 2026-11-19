//! The `entity_snapshots` table, which holds what an entity contained before a migration job overwrote it.
//! Reads and writes live in `entity_entities`, which owns the job's undo.

pub use crate::models::_entities::entity_snapshots::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, db: &C, _insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::sqlite_generated_id(db, this.id);
        Ok(this)
    }
}
