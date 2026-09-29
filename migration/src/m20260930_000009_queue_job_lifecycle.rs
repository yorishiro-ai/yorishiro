//! Persist the provider-neutral lifecycle observed around background jobs.

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
        let [created_at, updated_at] = helpers::timestamps(manager);
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("queue_job_lifecycles"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(ColumnDef::new(Alias::new("provider_job_id")).string().unique_key())
                    .col(ColumnDef::new(Alias::new("job_name")).string().not_null())
                    .col(ColumnDef::new(Alias::new("worker_class")).string().not_null())
                    .col(helpers::uuid_col(manager, Alias::new("workspace_id")))
                    .col(ColumnDef::new(Alias::new("plan")).string())
                    .col(ColumnDef::new(Alias::new("status")).string().not_null())
                    .col(helpers::ts_tz_col(manager, Alias::new("enqueue_at")).not_null())
                    .col(helpers::ts_tz_col(manager, Alias::new("claim_at")))
                    .col(helpers::ts_tz_col(manager, Alias::new("start_at")))
                    .col(helpers::ts_tz_col(manager, Alias::new("retry_at")))
                    .col(helpers::ts_tz_col(manager, Alias::new("completed_at")))
                    .col(helpers::ts_tz_col(manager, Alias::new("failed_at")))
                    .col(helpers::ts_tz_col(manager, Alias::new("cancelled_at")))
                    .col(ColumnDef::new(Alias::new("attempt")).integer().not_null().default(0))
                    .col(ColumnDef::new(Alias::new("concurrency_key")).string())
                    .col(ColumnDef::new(Alias::new("concurrency_limit")).integer())
                    .col(ColumnDef::new(Alias::new("error")).text())
                    .col(created_at)
                    .col(updated_at)
                    .check(Expr::cust("status IN ('queued', 'running', 'retrying', 'completed', 'failed', 'cancelled', 'unavailable')"))
                    .to_owned(),
            )
            .await?;
        for (name, column) in [
            (
                "queue_job_lifecycles_provider_job_id_idx",
                "provider_job_id",
            ),
            ("queue_job_lifecycles_status_class_idx", "status"),
            ("queue_job_lifecycles_enqueue_at_idx", "enqueue_at"),
        ] {
            manager
                .create_index(
                    Index::create()
                        .name(name)
                        .table(Alias::new("queue_job_lifecycles"))
                        .col(Alias::new(column))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("queue_job_lifecycles"))
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}
