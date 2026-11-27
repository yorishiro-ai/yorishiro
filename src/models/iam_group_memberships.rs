//! IAM group membership model: listing, adding and removing group members.

pub use crate::models::_entities::iam_group_memberships::{ActiveModel, Column, Entity, Model};
use sea_orm::{ActiveValue::Set, QueryFilter, QueryOrder, QuerySelect, entity::prelude::*};

use crate::error::{ResultExt, YorishiroError};
use crate::models::iam::policy::{Action, Resource};

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

impl Model {
    /// The users that belong to a group, in the order they joined.
    pub(crate) async fn members(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        group_id: Uuid,
    ) -> Result<Vec<crate::models::user_users::Model>, YorishiroError> {
        let ids: Vec<Uuid> = Entity::find()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::GroupId.eq(group_id))
            .order_by_asc(Column::Id)
            .all(conn)
            .await
            .internal()?
            .into_iter()
            .map(|row| row.user_id)
            .collect();
        let mut users = crate::models::user_users::Entity::find()
            .filter(crate::models::_entities::user_users::Column::Id.is_in(ids.clone()))
            .all(conn)
            .await
            .internal()?;
        users.sort_by_key(|user| ids.iter().position(|id| *id == user.id));
        Ok(users)
    }

    pub(crate) async fn count(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        group_id: Uuid,
    ) -> Result<u64, YorishiroError> {
        Entity::find()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::GroupId.eq(group_id))
            .count(conn)
            .await
            .internal()
    }

    /// The groups a user belongs to.
    pub(crate) async fn groups_for_user_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        user_id: Uuid,
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<crate::models::iam_groups::Model>, YorishiroError> {
        use crate::models::iam_groups;
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::ListGroupsForUser,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if !crate::models::tenant_memberships::is_member(&txn, actor.tenant_id, user_id).await? {
            return Err(YorishiroError::not_found("user was not found"));
        }
        let ids: Vec<Uuid> = Entity::find()
            .filter(Column::TenantId.eq(actor.tenant_id))
            .filter(Column::UserId.eq(user_id))
            .all(&txn)
            .await
            .internal()?
            .into_iter()
            .map(|row| row.group_id)
            .collect();
        let groups = iam_groups::Entity::find()
            .filter(iam_groups::Column::TenantId.eq(actor.tenant_id))
            .filter(iam_groups::Column::Id.is_in(ids))
            .order_by_asc(iam_groups::Column::NameKey)
            .limit(params.limit())
            .offset(params.offset())
            .all(&txn)
            .await
            .internal()?;
        txn.commit().await.internal()?;
        Ok(groups)
    }

    pub(crate) async fn add_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        group_id: Uuid,
        user_id: Uuid,
    ) -> Result<(), YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::AddUserToGroup,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if crate::models::iam_groups::Model::find(&txn, actor.tenant_id, group_id)
            .await?
            .is_none()
        {
            return Err(YorishiroError::not_found("IAM group was not found"));
        }
        if !crate::models::tenant_memberships::is_member(&txn, actor.tenant_id, user_id).await? {
            return Err(YorishiroError::not_found("user was not found"));
        }
        if crate::models::iam_group_policy_attachments::Model::carries_administrator(
            &txn,
            actor.tenant_id,
            group_id,
        )
        .await?
        {
            crate::models::iam::access::require_administrator(&txn, actor).await?;
        }
        Self::add(&txn, actor.tenant_id, group_id, user_id).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            Action::AddUserToGroup,
            serde_json::json!({"group_id": group_id, "user_id": user_id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn remove_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        group_id: Uuid,
        user_id: Uuid,
    ) -> Result<(), YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::RemoveUserFromGroup,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if crate::models::iam_groups::Model::find(&txn, actor.tenant_id, group_id)
            .await?
            .is_none()
        {
            return Err(YorishiroError::not_found("IAM group was not found"));
        }
        if !crate::models::tenant_memberships::is_member(&txn, actor.tenant_id, user_id).await? {
            return Err(YorishiroError::not_found("user was not found"));
        }
        if !Self::remove(&txn, actor.tenant_id, group_id, user_id).await? {
            return Err(YorishiroError::not_found("group association was not found"));
        }
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            Action::RemoveUserFromGroup,
            serde_json::json!({"group_id": group_id, "user_id": user_id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn add(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        group_id: Uuid,
        user_id: Uuid,
    ) -> Result<Self, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        if crate::models::iam_tenant_roots::Model::contains(txn, tenant_id, user_id).await? {
            return Err(YorishiroError::Conflict {
                message: "the tenant root cannot join IAM groups".into(),
            });
        }
        ActiveModel {
            tenant_id: Set(tenant_id),
            group_id: Set(group_id),
            user_id: Set(user_id),
            ..Default::default()
        }
        .insert(txn)
        .await
        .map_err(|error| {
            YorishiroError::conflict_on_unique(error, "group association already exists")
        })
    }

    pub(crate) async fn remove(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        group_id: Uuid,
        user_id: Uuid,
    ) -> Result<bool, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        Ok(Entity::delete_many()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::GroupId.eq(group_id))
            .filter(Column::UserId.eq(user_id))
            .exec(txn)
            .await
            .internal()?
            .rows_affected
            != 0)
    }
}
