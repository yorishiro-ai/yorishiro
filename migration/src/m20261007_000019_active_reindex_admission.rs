//! Prevent two processes from admitting an active reindex for one workspace.

use super::helpers;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name("queue_reindex_active_workspace_idx")
                    .unique()
                    .table(Alias::new("queue_job_lifecycles"))
                    .col(Alias::new("job_name"))
                    .col(Alias::new("workspace_id"))
                    .and_where(
                        Expr::col(Alias::new("job_name"))
                            .eq("reindex")
                            .and(Expr::col(Alias::new("workspace_id")).is_not_null())
                            .and(
                                Expr::col(Alias::new("status"))
                                    .is_in(["queued", "running", "retrying"]),
                            ),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("queue_reindex_active_workspace_idx")
                    .table(Alias::new("queue_job_lifecycles"))
                    .to_owned(),
            )
            .await
    }
}
