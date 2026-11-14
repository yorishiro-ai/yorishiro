pub use crate::models::_entities::iam_groups::{ActiveModel, Column, Entity, Model};
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
        use crate::models::iam_policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::GroupList,
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
            Action::GroupGet,
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
        use crate::models::iam_policy::{Action, Resource};
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            Action::GroupUpdate,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(YorishiroError::ValidationFailed {
                message: "group name must be between 1 and 100 characters".into(),
                details: vec![],
                hint: "provide a non-empty name of at most 100 characters".into(),
            });
        }
        if !Self::rename(&txn, actor.tenant_id, id, name.to_owned()).await? {
            return Err(YorishiroError::not_found("IAM group was not found"));
        }
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "group_update",
            serde_json::json!({"group_id":id}),
        )
        .await?;
        let group = Self::find(&txn, actor.tenant_id, id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("IAM group was not found"))?;
        txn.commit().await.internal()?;
        Ok(group)
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
            Action::GroupDelete,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if !Self::delete(&txn, actor.tenant_id, id).await? {
            return Err(YorishiroError::not_found("IAM group was not found"));
        }
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "group_delete",
            serde_json::json!({"group_id":id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn create_as(
        db: &sea_orm::DatabaseConnection,
        actor: &crate::models::api_keys::AuthContext,
        name: String,
    ) -> Result<Self, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            crate::models::api_keys::ApiKeyScope::Read,
            crate::models::iam_policy::Action::GroupCreate,
            crate::models::iam_policy::Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(YorishiroError::ValidationFailed {
                message: "group name must be between 1 and 100 characters".into(),
                details: vec![],
                hint: "provide a non-empty name of at most 100 characters".into(),
            });
        }
        let group = Self::create(&txn, actor.tenant_id, name.to_owned()).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "group_create",
            serde_json::json!({"group_id": group.id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(group)
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
            if matches!(
                error.sql_err(),
                Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
            ) {
                YorishiroError::Conflict {
                    message: "an IAM group with this name already exists".into(),
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
                if matches!(
                    error.sql_err(),
                    Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
                ) {
                    YorishiroError::Conflict {
                        message: "an IAM group with this name already exists".into(),
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
