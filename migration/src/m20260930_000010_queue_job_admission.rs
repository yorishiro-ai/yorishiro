//! Adds the durable reservation boundary used before provider dispatch.

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
            .alter_table(
                Table::alter()
                    .table(Alias::new("queue_job_lifecycles"))
                    .add_column(helpers::ts_tz_col(manager, Alias::new("admitted_at")))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("queue_job_lifecycles"))
                    .drop_column(Alias::new("admitted_at"))
                    .to_owned(),
            )
            .await
    }
}
