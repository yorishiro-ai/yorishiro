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
            .create_table(
                Table::create()
                    .table(Alias::new("queue_job_dispatch_outbox"))
                    .col(
                        helpers::uuid_col(manager, Alias::new("id"))
                            .not_null()
                            .primary_key(),
                    )
                    .col(helpers::uuid_col(manager, Alias::new("lifecycle_id")))
                    .col(helpers::json_col(manager, Alias::new("payload")).not_null())
                    .col(
                        ColumnDef::new(Alias::new("worker_name"))
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("queue_name")).string())
                    .col(helpers::json_col(manager, Alias::new("tags")))
                    .col(helpers::ts_tz_col(manager, Alias::new("last_attempt_at")))
                    .col(helpers::ts_tz_col(manager, Alias::new("deadline_at")))
                    .col(helpers::ts_tz_col(manager, Alias::new("published_at")))
                    .col(ColumnDef::new(Alias::new("claim_token")).string())
                    .col(helpers::ts_tz_col(manager, Alias::new("claim_until")))
                    .col(ColumnDef::new(Alias::new("priority")).integer())
                    .col(helpers::uuid_col(manager, Alias::new("workspace_id")))
                    .to_owned(),
            )
            .await?;
        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "queue_job_dispatch_outbox",
        )
        .await?;
        helpers::enable_rls_with_policy(
            manager,
            "queue_job_dispatch_outbox",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            true,
        )
        .await?;
        manager
            .create_index(
                Index::create()
                    .name("queue_job_dispatch_outbox_lifecycle_idx")
                    .table(Alias::new("queue_job_dispatch_outbox"))
                    .col(Alias::new("lifecycle_id"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("queue_job_dispatch_outbox"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
