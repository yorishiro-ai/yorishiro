pub use crate::models::_entities::iam_tenant_roots::{ActiveModel, Column, Entity, Model};
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue::Set, DatabaseTransaction};

use crate::error::{ResultExt, YorishiroError};

impl ActiveModelBehavior for ActiveModel {}

impl Model {
    pub(crate) async fn create(
        txn: &DatabaseTransaction,
        tenant_id: Uuid,
        user_id: Uuid,
    ) -> Result<(), YorishiroError> {
        ActiveModel {
            tenant_id: Set(tenant_id),
            user_id: Set(user_id),
        }
        .insert(txn)
        .await
        .internal()?;
        Ok(())
    }

    pub(crate) async fn contains(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        user_id: Uuid,
    ) -> Result<bool, YorishiroError> {
        Ok(Entity::find_by_id(tenant_id)
            .one(conn)
            .await
            .internal()?
            .is_some_and(|root| root.user_id == user_id))
    }
}
