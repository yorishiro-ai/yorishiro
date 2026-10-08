//! Stores enough worker arguments to recover queue dispatches after process or provider failures.

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
                    .if_not_exists()
                    .col(helpers::uuid_col(manager, Alias::new("lifecycle_id")).primary_key())
                    .col(helpers::json_col(manager, Alias::new("payload")).not_null())
                    .col(
                        ColumnDef::new(Alias::new("worker_name"))
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("queue_name")).string())
                    .col(helpers::json_col(manager, Alias::new("tags")))
                    .col(helpers::ts_tz_col(manager, Alias::new("last_attempt_at")))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("queue_job_dispatch_outbox"))
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}
