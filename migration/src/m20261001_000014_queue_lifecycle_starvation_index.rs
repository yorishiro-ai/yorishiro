//! Index lifecycle rows used by admission-side starvation protection.

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
                    .name("queue_job_lifecycles_starvation_idx")
                    .table(Alias::new("queue_job_lifecycles"))
                    .col(Alias::new("status"))
                    .col(Alias::new("worker_class"))
                    .col(Alias::new("enqueue_at"))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("queue_job_lifecycles_starvation_idx")
                    .table(Alias::new("queue_job_lifecycles"))
                    .to_owned(),
            )
            .await
    }
}
