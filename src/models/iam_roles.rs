//! Persistence for legacy IAM role data.

use sea_orm::entity::prelude::*;

pub use crate::models::_entities::iam_roles::{ActiveModel, Column, Entity, Model};

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(mut self, db: &C, _: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        self.id = crate::db::sqlite_generated_id(db, self.id);
        Ok(self)
    }
}
