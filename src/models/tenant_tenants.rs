pub use crate::models::_entities::tenant_tenants::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

mod operations;

pub(crate) use operations::count_tenants;
pub use operations::create_tenant;

/// The nil UUID reserved for infrastructure tenants that own no members and no data of their own.
pub(crate) const INFRASTRUCTURE_TENANT_ID: Uuid = Uuid::nil();

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    /// `id` has a `uuidv7()` column default on PostgreSQL and no default on SQLite; see `crate::db::sqlite_generated_id`.
    async fn before_save<C>(mut self, db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        self.id = crate::db::sqlite_generated_id(db, self.id);
        Ok(self)
    }
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}

pub(crate) async fn list_all(
    conn: &impl ConnectionTrait,
) -> Result<Vec<Model>, crate::error::YorishiroError> {
    use crate::error::ResultExt;

    Entity::find().all(conn).await.internal()
}
