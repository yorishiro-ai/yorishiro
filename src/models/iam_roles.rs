//! IAM role model: creating, renaming, reading and deleting roles.

pub use crate::models::_entities::iam_roles::{ActiveModel, Column, Entity, Model};
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
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<Self>, YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::RoleList,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let rows = Entity::find()
            .filter(Column::TenantId.eq(actor.tenant_id))
            .order_by_asc(Column::Id)
            .limit(params.limit())
            .offset(params.offset())
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
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::RoleGet,
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

    pub(crate) async fn rename_as(
        db: &DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        id: Uuid,
        name: String,
    ) -> Result<Self, YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::RoleUpdate,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if crate::models::iam::builtin::is_builtin_policy_name(name.trim()) {
            return Err(YorishiroError::Conflict {
                message: "this role name is reserved for a system-managed role".into(),
            });
        }
        let existing = Self::find(&txn, actor.tenant_id, id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("IAM role was not found"))?;
        if crate::models::iam::builtin::is_builtin_policy_name(&existing.name) {
            return Err(YorishiroError::Conflict {
                message: "system-managed roles cannot be changed".into(),
            });
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(YorishiroError::ValidationFailed {
                message: "role name must be between 1 and 100 characters".into(),
                details: vec![],
                hint: "provide a non-empty name of at most 100 characters".into(),
            });
        }
        if !Self::rename(&txn, actor.tenant_id, id, name.to_owned()).await? {
            return Err(YorishiroError::not_found("IAM role was not found"));
        }
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "role_update",
            serde_json::json!({"role_id":id}),
        )
        .await?;
        let role = Self::find(&txn, actor.tenant_id, id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("IAM role was not found"))?;
        txn.commit().await.internal()?;
        Ok(role)
    }

    pub(crate) async fn delete_as(
        db: &DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::iam::policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::RoleDelete,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let existing = Self::find(&txn, actor.tenant_id, id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("IAM role was not found"))?;
        if crate::models::iam::builtin::is_builtin_policy_name(&existing.name) {
            return Err(YorishiroError::Conflict {
                message: "system-managed roles cannot be changed".into(),
            });
        }
        if !Self::delete(&txn, actor.tenant_id, id).await? {
            return Err(YorishiroError::not_found("IAM role was not found"));
        }
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "role_delete",
            serde_json::json!({"role_id":id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn create_as(
        db: &DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        name: String,
    ) -> Result<Self, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            crate::models::iam::policy::Action::RoleCreate,
            crate::models::iam::policy::Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if crate::models::iam::builtin::is_builtin_policy_name(name.trim()) {
            return Err(YorishiroError::Conflict {
                message: "this role name is reserved for a system-managed role".into(),
            });
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(YorishiroError::ValidationFailed {
                message: "role name must be between 1 and 100 characters".into(),
                details: vec![],
                hint: "provide a non-empty name of at most 100 characters".into(),
            });
        }
        let role = Self::create(&txn, actor.tenant_id, name.to_owned()).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "role_create",
            serde_json::json!({"role_id": role.id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(role)
    }

    pub(crate) async fn create(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        name: String,
    ) -> Result<Self, YorishiroError> {
        ActiveModel {
            tenant_id: Set(tenant_id),
            name: Set(name),
            ..Default::default()
        }
        .insert(txn)
        .await
        .map_err(|error| {
            if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
                YorishiroError::Conflict {
                    message: "an IAM role with this name already exists".into(),
                }
            } else {
                YorishiroError::Internal(error.into())
            }
        })
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

    pub(crate) async fn rename(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        id: Uuid,
        name: String,
    ) -> Result<bool, YorishiroError> {
        use sea_orm::sea_query::Expr;
        Ok(Entity::update_many()
            .col_expr(Column::Name, Expr::value(name))
            .col_expr(
                Column::UpdatedAt,
                Expr::value(chrono::Utc::now().fixed_offset()),
            )
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::Id.eq(id))
            .exec(txn)
            .await
            .map_err(|error| {
                if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
                    YorishiroError::Conflict {
                        message: "an IAM role with this name already exists".into(),
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
