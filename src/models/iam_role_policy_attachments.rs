//! IAM model for attaching policies to roles.

pub use crate::models::_entities::iam_role_policy_attachments::{
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
        role_id: Uuid,
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<Self>, YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::RolePolicyList,
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
        policy_id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::RolePolicyAttach,
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
        if crate::models::iam_policies::Model::find(&txn, actor.tenant_id, policy_id)
            .await?
            .is_none()
        {
            return Err(YorishiroError::not_found("IAM policy was not found"));
        }
        let role = crate::models::iam_roles::Model::find(&txn, actor.tenant_id, role_id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("IAM role was not found"))?;
        if crate::models::iam::builtin::is_builtin_policy_name(&role.name) {
            return Err(YorishiroError::Conflict {
                message: "system-managed roles cannot be changed".into(),
            });
        }
        Self::add(&txn, actor.tenant_id, role_id, policy_id)
            .await
            .map_err(|error| match error {
                YorishiroError::Internal(ref source)
                    if source.downcast_ref::<DbErr>().is_some_and(|e| {
                        matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_)))
                    }) =>
                {
                    YorishiroError::Conflict {
                        message: "role association already exists".into(),
                    }
                }
                other => other,
            })?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "role_policy_attach",
            serde_json::json!({"role_id":role_id,"policy_id":policy_id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn remove_as(
        db: &DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        role_id: Uuid,
        policy_id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::RolePolicyDetach,
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
        if crate::models::iam_policies::Model::find(&txn, actor.tenant_id, policy_id)
            .await?
            .is_none()
        {
            return Err(YorishiroError::not_found("IAM policy was not found"));
        }
        let role = crate::models::iam_roles::Model::find(&txn, actor.tenant_id, role_id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("IAM role was not found"))?;
        if crate::models::iam::builtin::is_builtin_policy_name(&role.name) {
            return Err(YorishiroError::Conflict {
                message: "system-managed roles cannot be changed".into(),
            });
        }
        if !Self::remove(&txn, actor.tenant_id, role_id, policy_id).await? {
            return Err(YorishiroError::not_found("role association was not found"));
        }
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "role_policy_detach",
            serde_json::json!({"role_id":role_id,"policy_id":policy_id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn add(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        role_id: Uuid,
        policy_id: Uuid,
    ) -> Result<Self, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        let attached = Entity::find()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::RoleId.eq(role_id))
            .count(txn)
            .await
            .internal()?;
        if attached >= 10 {
            return Err(YorishiroError::Conflict {
                message: "a role can have at most 10 managed policies attached".into(),
            });
        }
        ActiveModel {
            tenant_id: Set(tenant_id),
            role_id: Set(role_id),
            policy_id: Set(policy_id),
            ..Default::default()
        }
        .insert(txn)
        .await
        .internal()
    }

    pub(crate) async fn remove(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        role_id: Uuid,
        policy_id: Uuid,
    ) -> Result<bool, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        Ok(Entity::delete_many()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::RoleId.eq(role_id))
            .filter(Column::PolicyId.eq(policy_id))
            .exec(txn)
            .await
            .internal()?
            .rows_affected
            != 0)
    }
}
