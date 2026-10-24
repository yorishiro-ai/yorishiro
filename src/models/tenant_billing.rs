use sea_orm::entity::prelude::*;

pub use crate::models::_entities::tenant_billing::{ActiveModel, Entity, Model};

#[loco_rs::prelude::async_trait]
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

impl Model {}

impl ActiveModel {}

impl Entity {}
