//! The body of an import, held until its worker has applied it.
//!
//! The queue payload names a row here and never carries the body, so the outbox, the queue and the logs never hold entry data.

pub use crate::models::_entities::import_staging::{ActiveModel, Entity, Model};
use sea_orm::{ActiveValue::Set, ConnectionTrait, EntityTrait, entity::prelude::*};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::generated_id(this.id);
        this.created_at = crate::db::stamped_created_at(insert, this.created_at);
        Ok(this)
    }
}

/// Stores an import body on the transaction that enqueues its job.
pub(crate) async fn stage(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    workspace_id: Uuid,
    body: String,
) -> Result<Uuid, YorishiroError> {
    Ok(ActiveModel {
        tenant_id: Set(tenant_id),
        workspace_id: Set(workspace_id),
        body: Set(body),
        ..Default::default()
    }
    .insert(conn)
    .await
    .internal()?
    .id)
}

/// The staged body, or `None` once the import that held it has completed.
pub(crate) async fn body(
    conn: &impl ConnectionTrait,
    id: Uuid,
) -> Result<Option<String>, YorishiroError> {
    Ok(Entity::find_by_id(id)
        .one(conn)
        .await
        .internal()?
        .map(|row| row.body))
}

/// Removes a body that has been applied.
pub(crate) async fn remove(conn: &impl ConnectionTrait, id: Uuid) -> Result<(), YorishiroError> {
    Entity::delete_by_id(id).exec(conn).await.internal()?;
    Ok(())
}

/// Removes bodies whose job never completed, so a failed import does not keep its data indefinitely.
pub(crate) async fn purge_before(
    conn: &impl ConnectionTrait,
    cutoff: chrono::DateTime<chrono::FixedOffset>,
) -> Result<u64, YorishiroError> {
    use crate::models::_entities::import_staging::Column;
    Ok(Entity::delete_many()
        .filter(Column::CreatedAt.lt(cutoff))
        .exec(conn)
        .await
        .internal()?
        .rows_affected)
}
