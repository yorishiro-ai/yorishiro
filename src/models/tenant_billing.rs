#[cfg(feature = "enterprise")]
use sea_orm::QuerySelect;
use sea_orm::entity::prelude::*;

#[cfg(feature = "enterprise")]
use crate::error::{ResultExt, YorishiroError};
pub use crate::models::_entities::tenant_billing::{ActiveModel, Entity, Model};

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

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}

#[cfg(feature = "enterprise")]
pub(crate) async fn find_plan(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<Option<Option<String>>, YorishiroError> {
    Entity::find_by_id(tenant_id)
        .select_only()
        .column(crate::models::_entities::tenant_billing::Column::Plan)
        .into_tuple()
        .one(conn)
        .await
        .internal()
}
