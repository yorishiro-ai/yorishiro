//! IAM model for attaching policies to roles.

pub use crate::models::_entities::iam_role_policy_attachments::{
    ActiveModel, Column, Entity, Model,
};
use sea_orm::{ActiveValue::Set, QueryFilter, QueryOrder, QuerySelect, entity::prelude::*};

use crate::error::{ResultExt, YorishiroError};
use crate::models::api_keys::{ApiKeyScope, AuthContext};
use crate::models::iam_policy::{Action, Resource};

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
        actor: &AuthContext,
        role_id: Uuid,
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<Self>, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            ApiKeyScope::Read,
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
            .limit(params.limit() as u64)
            .offset(params.offset() as u64)
            .all(&txn)
            .await
            .internal()?;
        txn.commit().await.internal()?;
        Ok(rows)
    }

    pub(crate) async fn add_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
        role_id: Uuid,
        policy_id: Uuid,
    ) -> Result<(), YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            ApiKeyScope::Read,
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
        crate::db::lock_for_update(&txn, &format!("tenant-membership:{}", actor.tenant_id))
            .await
            .internal()?;
        let attached = Entity::find()
            .filter(Column::TenantId.eq(actor.tenant_id))
            .filter(Column::RoleId.eq(role_id))
            .count(&txn)
            .await
            .internal()?;
        if attached >= crate::models::iam_roles::MAX_POLICIES_PER_ROLE {
            return Err(YorishiroError::Conflict {
                message: "a role can have at most 20 managed policies attached".into(),
            });
        }
        ActiveModel {
            tenant_id: Set(actor.tenant_id),
            role_id: Set(role_id),
            policy_id: Set(policy_id),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .map_err(|error| {
            if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
                YorishiroError::Conflict {
                    message: "role association already exists".into(),
                }
            } else {
                YorishiroError::Internal(error.into())
            }
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
        actor: &AuthContext,
        role_id: Uuid,
        policy_id: Uuid,
    ) -> Result<(), YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            ApiKeyScope::Read,
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
        crate::db::lock_for_update(&txn, &format!("tenant-membership:{}", actor.tenant_id))
            .await
            .internal()?;
        let removed = Entity::delete_many()
            .filter(Column::TenantId.eq(actor.tenant_id))
            .filter(Column::RoleId.eq(role_id))
            .filter(Column::PolicyId.eq(policy_id))
            .exec(&txn)
            .await
            .internal()?
            .rows_affected;
        if removed == 0 {
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
}
