//! IAM group model: creating, renaming, reading and deleting groups.
//! A group name is unique within its tenant regardless of case.

use crate::error::OptionExt;
pub use crate::models::_entities::iam_groups::{ActiveModel, Column, Entity, Model};
use sea_orm::{ActiveValue::Set, QueryFilter, QueryOrder, QuerySelect, entity::prelude::*};

use crate::error::{ResultExt, YorishiroError};
use crate::models::iam::names;
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
        this.updated_at = crate::db::stamped_updated_at(this.updated_at);
        Ok(this)
    }
}

/// A group with the users that belong to it.
#[derive(Debug)]
pub struct GroupView {
    pub group: Model,
    pub members: Vec<crate::models::user_users::Model>,
}

impl Model {
    #[must_use]
    pub fn yrn(&self) -> String {
        names::group_yrn(self.tenant_id, &self.name)
    }

    pub(crate) async fn list_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<Self>, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::ListGroups,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let rows = Entity::find()
            .filter(Column::TenantId.eq(actor.tenant_id))
            .order_by_asc(Column::NameKey)
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
        actor: &crate::models::access_keys::AuthContext,
        id: Uuid,
    ) -> Result<GroupView, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::GetGroup,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let group = Self::find(&txn, actor.tenant_id, id)
            .await?
            .or_not_found("IAM group was not found")?;
        let members =
            crate::models::iam_group_memberships::Model::members(&txn, actor.tenant_id, id).await?;
        txn.commit().await.internal()?;
        Ok(GroupView { group, members })
    }

    pub(crate) async fn rename_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        id: Uuid,
        name: String,
    ) -> Result<Self, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::UpdateGroup,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        names::validate_name("group", &name, names::MAX_GROUP_NAME_CHARS)?;
        let before = Self::find(&txn, actor.tenant_id, id)
            .await?
            .or_not_found("IAM group was not found")?;
        Self::rename(&txn, actor.tenant_id, id, name.clone()).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            Action::UpdateGroup,
            serde_json::json!({"group_id": id, "before": before.name, "after": name}),
        )
        .await?;
        let group = Self::find(&txn, actor.tenant_id, id)
            .await?
            .or_not_found("IAM group was not found")?;
        txn.commit().await.internal()?;
        Ok(group)
    }

    /// Deletes a group that has no members and no attached policies.
    pub(crate) async fn delete_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        id: Uuid,
    ) -> Result<(), YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::DeleteGroup,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let group = Self::find(&txn, actor.tenant_id, id)
            .await?
            .or_not_found("IAM group was not found")?;
        crate::db::lock_for_update(&txn, &format!("tenant-membership:{}", actor.tenant_id))
            .await
            .internal()?;
        let members =
            crate::models::iam_group_memberships::Model::count(&txn, actor.tenant_id, id).await?;
        let policies =
            crate::models::iam_group_policy_attachments::Model::count(&txn, actor.tenant_id, id)
                .await?;
        if members > 0 || policies > 0 {
            return Err(YorishiroError::Conflict {
                message: format!(
                    "the group still has {members} member(s) and {policies} attached policy(ies); remove them first"
                ),
            });
        }
        Self::delete(&txn, actor.tenant_id, id).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            Action::DeleteGroup,
            serde_json::json!({"group_id": id, "name": group.name}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn create_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        name: String,
    ) -> Result<Self, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::CreateGroup,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        names::validate_name("group", &name, names::MAX_GROUP_NAME_CHARS)?;
        let group = Self::create(&txn, actor.tenant_id, name).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            Action::CreateGroup,
            serde_json::json!({"group_id": group.id, "name": group.name}),
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
            name_key: Set(names::name_key(&name)),
            name: Set(name),
            ..Default::default()
        }
        .insert(txn)
        .await
        .map_err(|error| {
            YorishiroError::conflict_on_unique(error, "an IAM group with this name already exists")
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
            .col_expr(Column::NameKey, Expr::value(names::name_key(&name)))
            .col_expr(Column::Name, Expr::value(name))
            .col_expr(
                Column::UpdatedAt,
                Expr::value(crate::data::time::now().fixed_offset()),
            )
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::Id.eq(id))
            .exec(txn)
            .await
            .map_err(|error| {
                YorishiroError::conflict_on_unique(
                    error,
                    "an IAM group with this name already exists",
                )
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
