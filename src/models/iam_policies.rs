pub use crate::models::_entities::iam_policies::{ActiveModel, Column, Entity, Model};
use sea_orm::{ActiveValue::Set, QueryFilter, QueryOrder, QuerySelect, entity::prelude::*};

use crate::error::{ResultExt, YorishiroError};
use crate::models::{_entities::workspace_workspaces, iam_policy::Policy};

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

#[allow(dead_code)]
async fn validate(
    txn: &sea_orm::DatabaseTransaction,
    tenant_id: Uuid,
    document: &serde_json::Value,
) -> Result<(), YorishiroError> {
    let json = serde_json::to_string(document).internal()?;
    let ids = Policy::workspace_ids(&json)?;
    let owned = workspace_workspaces::Entity::find()
        .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
        .filter(workspace_workspaces::Column::Id.is_in(ids))
        .all(txn)
        .await
        .internal()?;
    Policy::parse(&json, tenant_id, |tenant, workspace| {
        tenant == tenant_id && owned.iter().any(|w| w.id == workspace)
    })?;
    Ok(())
}

// Persistence is internal until authorization and audit are wired.
#[allow(dead_code)]
impl Model {
    pub(crate) async fn list_as(
        db: &DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<Self>, YorishiroError> {
        use crate::models::iam_policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::PolicyList,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let rows = Entity::find()
            .filter(Column::TenantId.eq(actor.tenant_id))
            .order_by_asc(Column::Id)
            .limit(params.limit() as u64)
            .offset(params.offset() as u64)
            .all(&txn)
            .await
            .internal()?;
        txn.commit().await.internal()?;
        Ok(rows)
    }
    pub(crate) async fn get_as(
        db: &DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        id: Uuid,
    ) -> Result<Self, YorishiroError> {
        use crate::models::iam_policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::PolicyGet,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let row = Self::find(&txn, actor.tenant_id, id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("IAM resource was not found"))?;
        txn.commit().await.internal()?;
        Ok(row)
    }

    pub(crate) async fn update_as(
        db: &DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        id: Uuid,
        name: String,
        document: serde_json::Value,
    ) -> Result<Self, YorishiroError> {
        use crate::models::iam_policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::PolicyUpdate,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let before = Self::find(&txn, actor.tenant_id, id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("IAM policy was not found"))?;
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(YorishiroError::ValidationFailed {
                message: "policy name must be between 1 and 100 characters".into(),
                details: vec![],
                hint: "provide a non-empty name of at most 100 characters".into(),
            });
        }
        Self::update(&txn, actor.tenant_id, id, name.to_owned(), document).await?;
        let policy = Self::find(&txn, actor.tenant_id, id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("IAM policy was not found"))?;
        crate::models::iam_audit_events::record(&txn, actor, "policy_update",
            serde_json::json!({"policy_id":id,"before":{"name":before.name,"document":before.document},
                "after":{"name":policy.name,"document":policy.document}})).await?;
        txn.commit().await.internal()?;
        Ok(policy)
    }

    pub(crate) async fn delete_as(
        db: &DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::iam_policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::PolicyDelete,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let policy = Self::find(&txn, actor.tenant_id, id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("IAM policy was not found"))?;
        Self::delete(&txn, actor.tenant_id, id).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "policy_delete",
            serde_json::json!({"policy_id":id,"name":policy.name,"document":policy.document}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn create_as(
        db: &sea_orm::DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        name: String,
        document: serde_json::Value,
    ) -> Result<Self, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            crate::models::iam_policy::Action::PolicyCreate,
            crate::models::iam_policy::Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(YorishiroError::ValidationFailed {
                message: "policy name must be between 1 and 100 characters".into(),
                details: vec![],
                hint: "provide a non-empty name of at most 100 characters".into(),
            });
        }
        let policy = Self::create(&txn, actor.tenant_id, name.to_owned(), document).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "policy_create",
            serde_json::json!({"policy_id": policy.id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(policy)
    }

    pub(crate) async fn for_user(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        user_id: Uuid,
    ) -> Result<Vec<Self>, YorishiroError> {
        use crate::models::_entities::{
            iam_group_memberships, iam_group_policy_attachments, iam_user_policy_attachments,
        };

        let direct = iam_user_policy_attachments::Entity::find()
            .filter(iam_user_policy_attachments::Column::TenantId.eq(tenant_id))
            .filter(iam_user_policy_attachments::Column::UserId.eq(user_id))
            .all(txn)
            .await
            .internal()?;
        let groups = iam_group_memberships::Entity::find()
            .filter(iam_group_memberships::Column::TenantId.eq(tenant_id))
            .filter(iam_group_memberships::Column::UserId.eq(user_id))
            .all(txn)
            .await
            .internal()?;
        let attached = iam_group_policy_attachments::Entity::find()
            .filter(iam_group_policy_attachments::Column::TenantId.eq(tenant_id))
            .filter(
                iam_group_policy_attachments::Column::GroupId
                    .is_in(groups.into_iter().map(|group| group.group_id)),
            )
            .all(txn)
            .await
            .internal()?;
        let ids = direct
            .into_iter()
            .map(|attachment| attachment.policy_id)
            .chain(attached.into_iter().map(|attachment| attachment.policy_id));
        Entity::find()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::Id.is_in(ids))
            .all(txn)
            .await
            .internal()
    }

    pub(crate) async fn create(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        name: String,
        document: serde_json::Value,
    ) -> Result<Self, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        validate(txn, tenant_id, &document).await?;
        ActiveModel {
            tenant_id: Set(tenant_id),
            name: Set(name),
            document: Set(document),
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
                    message: "an IAM policy with this name already exists".into(),
                }
            } else {
                YorishiroError::Internal(error.into())
            }
        })
    }

    pub(crate) async fn list(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
    ) -> Result<Vec<Self>, YorishiroError> {
        Entity::find()
            .filter(Column::TenantId.eq(tenant_id))
            .all(txn)
            .await
            .internal()
    }

    pub(crate) async fn find(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        id: Uuid,
    ) -> Result<Option<Self>, YorishiroError> {
        Entity::find_by_id(id)
            .filter(Column::TenantId.eq(tenant_id))
            .one(txn)
            .await
            .internal()
    }

    pub(crate) async fn update(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        id: Uuid,
        name: String,
        document: serde_json::Value,
    ) -> Result<bool, YorishiroError> {
        use sea_orm::sea_query::Expr;
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        validate(txn, tenant_id, &document).await?;
        Ok(Entity::update_many()
            .col_expr(Column::Name, Expr::value(name))
            .col_expr(Column::Document, Expr::value(document))
            .col_expr(
                Column::UpdatedAt,
                Expr::value(chrono::Utc::now().fixed_offset()),
            )
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::Id.eq(id))
            .exec(txn)
            .await
            .map_err(|error| {
                if matches!(
                    error.sql_err(),
                    Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
                ) {
                    YorishiroError::Conflict {
                        message: "an IAM policy with this name already exists".into(),
                    }
                } else {
                    YorishiroError::Internal(error.into())
                }
            })?
            .rows_affected
            != 0)
    }

    pub(crate) async fn delete(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        id: Uuid,
    ) -> Result<bool, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        Ok(Entity::delete_many()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::Id.eq(id))
            .exec(txn)
            .await
            .internal()?
            .rows_affected
            != 0)
    }
}
