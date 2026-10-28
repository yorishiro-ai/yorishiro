pub use crate::models::_entities::workspace_memberships::{ActiveModel, Column, Entity, Model};
use sea_orm::DatabaseTransaction;
use sea_orm::entity::prelude::*;

use crate::db_enum::db_enum;
use crate::error::{ResultExt, YorishiroError};
use crate::models::api_keys::ApiKeyScope;

db_enum! {
    /// Workspace-local authority never grants tenant administration.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum WorkspaceRole {
        Admin = "admin",
        Member = "member",
        Viewer = "viewer",
    }
}

impl WorkspaceRole {
    pub fn max_scope(self) -> ApiKeyScope {
        match self {
            Self::Admin => ApiKeyScope::Migration,
            Self::Member => ApiKeyScope::Write,
            Self::Viewer => ApiKeyScope::Read,
        }
    }
}

impl Model {
    /// Records an explicit grant under the membership fence in the caller's transaction.
    ///
    /// # Errors
    /// Rejects users outside the workspace's tenant and propagates persistence failures.
    pub async fn grant(
        txn: &DatabaseTransaction,
        workspace_id: Uuid,
        user_id: Uuid,
        role: WorkspaceRole,
    ) -> Result<(), YorishiroError> {
        use sea_orm::sea_query::OnConflict;
        let tenant_id =
            crate::models::workspace_workspaces::get_workspace_tenant(txn, workspace_id).await?;
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        crate::models::tenant_memberships::get_membership_role(txn, tenant_id, user_id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("tenant membership not found"))?;
        let active = ActiveModel {
            id: crate::db::sqlite_generated_id(txn, sea_orm::ActiveValue::NotSet),
            workspace_id: sea_orm::ActiveValue::Set(workspace_id),
            user_id: sea_orm::ActiveValue::Set(user_id),
            role: sea_orm::ActiveValue::Set(role.as_db_str().into()),
            ..Default::default()
        };
        Entity::insert(active)
            .on_conflict(
                OnConflict::columns([Column::WorkspaceId, Column::UserId])
                    .update_column(Column::Role)
                    .to_owned(),
            )
            .exec(txn)
            .await
            .internal()?;
        Ok(())
    }

    /// Finds a direct workspace grant, independently of inherited tenant authority.
    ///
    /// # Errors
    /// Propagates lookup failures and rejects invalid persisted roles.
    pub async fn role(
        conn: &impl ConnectionTrait,
        workspace_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<WorkspaceRole>, YorishiroError> {
        let membership = Entity::find()
            .filter(Column::WorkspaceId.eq(workspace_id))
            .filter(Column::UserId.eq(user_id))
            .one(conn)
            .await
            .internal()?;
        membership
            .map(|membership| {
                WorkspaceRole::from_db_str(&membership.role).ok_or_else(|| {
                    YorishiroError::Internal(anyhow::anyhow!("unknown workspace membership role"))
                })
            })
            .transpose()
    }
}

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(mut self, db: &C, _insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        self.id = crate::db::sqlite_generated_id(db, self.id);
        Ok(self)
    }
}
