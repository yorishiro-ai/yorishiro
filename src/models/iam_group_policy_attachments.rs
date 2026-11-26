//! IAM model for attaching policies to groups.

use crate::error::OptionExt;
pub use crate::models::_entities::iam_group_policy_attachments::{
    ActiveModel, Column, Entity, Model,
};
use sea_orm::{ActiveValue::Set, QueryFilter, QueryOrder, QuerySelect, entity::prelude::*};

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

impl Model {
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

    /// Whether the group holds the managed `AdministratorAccess`.
    pub(crate) async fn carries_administrator(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        group_id: Uuid,
    ) -> Result<bool, YorishiroError> {
        let ids: Vec<Uuid> = Entity::find()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::GroupId.eq(group_id))
            .all(conn)
            .await
            .internal()?
            .into_iter()
            .map(|row| row.policy_id)
            .collect();
        Ok(
            crate::models::iam_policies::Model::attached(conn, tenant_id, ids)
                .await?
                .iter()
                .any(|policy| policy.name == crate::models::iam::builtin::ADMINISTRATOR_ACCESS),
        )
    }

    pub(crate) async fn list_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        group_id: Uuid,
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<crate::models::iam_policies::AttachedPolicy>, YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::ListAttachedGroupPolicies,
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
        let ids = Entity::find()
            .filter(Column::TenantId.eq(actor.tenant_id))
            .filter(Column::GroupId.eq(group_id))
            .order_by_asc(Column::Id)
            .limit(params.limit())
            .offset(params.offset())
            .all(&txn)
            .await
            .internal()?
            .into_iter()
            .map(|row| row.policy_id)
            .collect();
        let policies =
            crate::models::iam_policies::Model::attached(&txn, actor.tenant_id, ids).await?;
        txn.commit().await.internal()?;
        Ok(policies)
    }

    pub(crate) async fn add_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        group_id: Uuid,
        policy_id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::AttachGroupPolicy,
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
        let policy = crate::models::iam_policies::Model::find(&txn, actor.tenant_id, policy_id)
            .await?
            .or_not_found("IAM policy was not found")?;
        if policy.is_administrator_access() {
            crate::models::iam::access::require_administrator(&txn, actor).await?;
        }
        Self::add(&txn, actor.tenant_id, group_id, policy_id).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            Action::AttachGroupPolicy,
            serde_json::json!({"group_id": group_id, "policy_id": policy_id, "policy_name": policy.name}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn remove_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        group_id: Uuid,
        policy_id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::DetachGroupPolicy,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if crate::models::iam_policies::Model::find(&txn, actor.tenant_id, policy_id)
            .await?
            .is_none()
        {
            return Err(YorishiroError::not_found("IAM policy was not found"));
        }
        if !Self::remove(&txn, actor.tenant_id, group_id, policy_id).await? {
            return Err(YorishiroError::not_found("group association was not found"));
        }
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            Action::DetachGroupPolicy,
            serde_json::json!({"group_id": group_id, "policy_id": policy_id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn add(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        group_id: Uuid,
        policy_id: Uuid,
    ) -> Result<Self, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        let attached = Entity::find()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::GroupId.eq(group_id))
            .count(txn)
            .await
            .internal()?;
        if attached >= 10 {
            return Err(YorishiroError::Conflict {
                message: "a group can have at most 10 managed policies attached".into(),
            });
        }
        ActiveModel {
            tenant_id: Set(tenant_id),
            group_id: Set(group_id),
            policy_id: Set(policy_id),
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
        policy_id: Uuid,
    ) -> Result<bool, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        Ok(Entity::delete_many()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::GroupId.eq(group_id))
            .filter(Column::PolicyId.eq(policy_id))
            .exec(txn)
            .await
            .internal()?
            .rows_affected
            != 0)
    }
}
