pub use crate::models::_entities::tenant_tenants::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

mod operations;
pub mod retirement;

pub(crate) use operations::count_tenants;
pub use operations::create_tenant;

/// The nil UUID reserved for infrastructure tenants that own no members and no data of their own.
pub(crate) const INFRASTRUCTURE_TENANT_ID: Uuid = Uuid::nil();

#[loco_rs::prelude::async_trait]
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

impl Model {
    /// Fetches a tenant by its primary key.
    ///
    /// # Errors
    /// Returns an error if the tenant is missing or the query fails.
    pub async fn find_by_id(
        conn: &impl ConnectionTrait,
        id: Uuid,
    ) -> Result<Self, crate::error::YorishiroError> {
        use crate::error::{ResultExt, YorishiroError};

        Entity::find_by_id(id)
            .one(conn)
            .await
            .internal()?
            .ok_or_else(|| YorishiroError::not_found("tenant not found"))
    }
}

pub(crate) async fn list_all(
    conn: &impl ConnectionTrait,
) -> Result<Vec<Model>, crate::error::YorishiroError> {
    use crate::error::ResultExt;

    Entity::find().all(conn).await.internal()
}
