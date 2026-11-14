pub use crate::models::_entities::iam_user_policy_attachments::{
    ActiveModel, Column, Entity, Model,
};
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
        user_id: Uuid,
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<Self>, YorishiroError> {
        use crate::models::iam_policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::UserPolicyList,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if crate::models::tenant_memberships::get_membership_role(&txn, actor.tenant_id, user_id)
            .await?
            .is_none()
        {
            return Err(YorishiroError::not_found("tenant user was not found"));
        }
        let rows = Entity::find()
            .filter(Column::TenantId.eq(actor.tenant_id))
            .filter(Column::UserId.eq(user_id))
            .order_by_asc(Column::Id)
            .limit(params.limit() as u64)
            .offset(params.offset() as u64)
            .all(&txn)
            .await
            .internal()?;
        txn.commit().await.internal()?;
        Ok(rows)
    }

    pub(crate) async fn add_as(
        db: &sea_orm::DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        user_id: Uuid,
        policy_id: Uuid,
    ) -> Result<Self, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            crate::models::iam_policy::Action::UserPolicyAttach,
            crate::models::iam_policy::Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let attachment = Self::add(&txn, actor.tenant_id, user_id, policy_id).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "user_policy_attach",
            serde_json::json!({"user_id":user_id,"policy_id":policy_id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(attachment)
    }

    pub(crate) async fn remove_as(
        db: &DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        user_id: Uuid,
        policy_id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::iam_policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::UserPolicyDetach,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if !Self::remove(&txn, actor.tenant_id, user_id, policy_id).await? {
            return Err(YorishiroError::not_found(
                "user policy attachment was not found",
            ));
        }
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "user_policy_detach",
            serde_json::json!({"user_id":user_id,"policy_id":policy_id}),
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
        if crate::models::iam_tenant_roots::Model::contains(txn, tenant_id, user_id).await? {
            return Err(YorishiroError::Conflict {
                message: "policies cannot be attached to the tenant root".into(),
            });
        }
        if crate::models::tenant_memberships::get_membership_role(txn, tenant_id, user_id)
            .await?
            .is_none()
            || crate::models::iam_policies::Model::find(txn, tenant_id, policy_id)
                .await?
                .is_none()
        {
            return Err(YorishiroError::not_found(
                "tenant user or IAM policy was not found",
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
            if matches!(
                error.sql_err(),
                Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
            ) {
                YorishiroError::Conflict {
                    message: "policy is already attached to this user".into(),
                }
            } else {
                YorishiroError::Internal(error.into())
            }
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
