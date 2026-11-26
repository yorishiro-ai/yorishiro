//! IAM model for attaching policies directly to users.

use crate::error::OptionExt;
pub use crate::models::_entities::iam_user_policy_attachments::{
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
    pub(crate) async fn count_for_user(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        user_id: Uuid,
    ) -> Result<u64, YorishiroError> {
        Entity::find()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::UserId.eq(user_id))
            .count(conn)
            .await
            .internal()
    }

    pub(crate) async fn list_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        user_id: Uuid,
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<crate::models::iam_policies::AttachedPolicy>, YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::ListAttachedUserPolicies,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if !crate::models::tenant_memberships::is_member(&txn, actor.tenant_id, user_id).await? {
            return Err(YorishiroError::not_found("user was not found"));
        }
        let ids = Entity::find()
            .filter(Column::TenantId.eq(actor.tenant_id))
            .filter(Column::UserId.eq(user_id))
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
        user_id: Uuid,
        policy_id: Uuid,
    ) -> Result<(), YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::iam::policy::Action::AttachUserPolicy,
            crate::models::iam::policy::Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let policy = crate::models::iam_policies::Model::find(&txn, actor.tenant_id, policy_id)
            .await?
            .or_not_found("user or IAM policy was not found")?;
        if policy.is_administrator_access() {
            crate::models::iam::access::require_administrator(&txn, actor).await?;
        }
        Self::add(&txn, actor.tenant_id, user_id, policy_id).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            crate::models::iam::policy::Action::AttachUserPolicy,
            serde_json::json!({"user_id": user_id, "policy_id": policy_id, "policy_name": policy.name}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn remove_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        user_id: Uuid,
        policy_id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::DetachUserPolicy,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let policy = crate::models::iam_policies::Model::find(&txn, actor.tenant_id, policy_id)
            .await?
            .or_not_found("user policy attachment was not found")?;
        if !Self::remove(&txn, actor.tenant_id, user_id, policy_id).await? {
            return Err(YorishiroError::not_found(
                "user policy attachment was not found",
            ));
        }
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            Action::DetachUserPolicy,
            serde_json::json!({"user_id": user_id, "policy_id": policy_id, "policy_name": policy.name}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn add(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        user_id: Uuid,
        policy_id: Uuid,
    ) -> Result<Self, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        let attached = Entity::find()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::UserId.eq(user_id))
            .count(txn)
            .await
            .internal()?;
        if attached >= 10 {
            return Err(YorishiroError::Conflict {
                message: "a user can have at most 10 managed policies attached".into(),
            });
        }
        if crate::models::iam_tenant_roots::Model::contains(txn, tenant_id, user_id).await? {
            return Err(YorishiroError::Conflict {
                message: "policies cannot be attached to the tenant root".into(),
            });
        }
        if !crate::models::tenant_memberships::is_member(txn, tenant_id, user_id).await?
            || crate::models::iam_policies::Model::find(txn, tenant_id, policy_id)
                .await?
                .is_none()
        {
            return Err(YorishiroError::not_found(
                "user or IAM policy was not found",
            ));
        }
        ActiveModel {
            tenant_id: Set(tenant_id),
            user_id: Set(user_id),
            policy_id: Set(policy_id),
            ..Default::default()
        }
        .insert(txn)
        .await
        .map_err(|error| {
            YorishiroError::conflict_on_unique(error, "policy is already attached to this user")
        })
    }

    pub(crate) async fn remove(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        user_id: Uuid,
        policy_id: Uuid,
    ) -> Result<bool, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        Ok(Entity::delete_many()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::UserId.eq(user_id))
            .filter(Column::PolicyId.eq(policy_id))
            .exec(txn)
            .await
            .internal()?
            .rows_affected
            != 0)
    }
}
