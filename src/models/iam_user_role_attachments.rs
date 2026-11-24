//! IAM role membership model: listing, adding and removing role members.

pub use crate::models::_entities::iam_user_role_attachments::{ActiveModel, Column, Entity, Model};
use sea_orm::{ActiveValue::Set, QueryFilter, QueryOrder, QuerySelect, entity::prelude::*};

use crate::error::{ResultExt, YorishiroError};

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

impl Model {
    pub(crate) async fn list_as(
        db: &DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        role_id: Uuid,
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<Self>, YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::UserRoleList,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if crate::models::iam_roles::Model::find(&txn, actor.tenant_id, role_id)
            .await?
            .is_none()
        {
            return Err(YorishiroError::not_found("IAM role was not found"));
        }
        let rows = Entity::find()
            .filter(Column::TenantId.eq(actor.tenant_id))
            .filter(Column::RoleId.eq(role_id))
            .order_by_asc(Column::Id)
            .limit(params.limit())
            .offset(params.offset())
            .all(&txn)
            .await
            .internal()?;
        txn.commit().await.internal()?;
        Ok(rows)
    }

    pub(crate) async fn add_as(
        db: &DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        role_id: Uuid,
        user_id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::UserRoleAdd,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if crate::models::iam_roles::Model::find(&txn, actor.tenant_id, role_id)
            .await?
            .is_none()
        {
            return Err(YorishiroError::not_found("IAM role was not found"));
        }
        if !crate::models::tenant_memberships::is_member(&txn, actor.tenant_id, user_id).await? {
            return Err(YorishiroError::not_found("tenant user was not found"));
        }
        Self::add(&txn, actor.tenant_id, role_id, user_id).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "user_role_add",
            serde_json::json!({"role_id":role_id,"user_id":user_id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn remove_as(
        db: &DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        role_id: Uuid,
        user_id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::UserRoleRemove,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if crate::models::iam_roles::Model::find(&txn, actor.tenant_id, role_id)
            .await?
            .is_none()
        {
            return Err(YorishiroError::not_found("IAM role was not found"));
        }
        if !crate::models::tenant_memberships::is_member(&txn, actor.tenant_id, user_id).await? {
            return Err(YorishiroError::not_found("tenant user was not found"));
        }
        if !Self::remove(&txn, actor.tenant_id, role_id, user_id).await? {
            return Err(YorishiroError::not_found("role association was not found"));
        }
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "user_role_remove",
            serde_json::json!({"role_id":role_id,"user_id":user_id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn add(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        role_id: Uuid,
        user_id: Uuid,
    ) -> Result<Self, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        if crate::models::iam_tenant_roots::Model::contains(txn, tenant_id, user_id).await? {
            return Err(YorishiroError::Conflict {
                message: "the tenant root cannot join IAM roles".into(),
            });
        }
        ActiveModel {
            tenant_id: Set(tenant_id),
            role_id: Set(role_id),
            user_id: Set(user_id),
            ..Default::default()
        }
        .insert(txn)
        .await
        .map_err(|error| {
            YorishiroError::conflict_on_unique(error, "role association already exists")
        })
    }

    pub(crate) async fn remove(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        role_id: Uuid,
        user_id: Uuid,
    ) -> Result<bool, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        Ok(Entity::delete_many()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::RoleId.eq(role_id))
            .filter(Column::UserId.eq(user_id))
            .exec(txn)
            .await
            .internal()?
            .rows_affected
            != 0)
    }
}
