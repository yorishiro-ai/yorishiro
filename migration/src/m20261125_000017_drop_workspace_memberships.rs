//! Drops `workspace_memberships`: IAM policies are the only authority over a workspace.

use sea_orm_migration::prelude::*;

use crate::helpers;
use crate::m20261020_000001_workspaces::{down_workspace_memberships, up_workspace_memberships};

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        down_workspace_memberships(manager).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        up_workspace_memberships(manager).await
    }
}
