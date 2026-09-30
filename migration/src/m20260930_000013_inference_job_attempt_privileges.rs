//! Grants the tenant role the inference-job privileges required by proposal writes.

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
        helpers::grant(manager, "SELECT, UPDATE", "inference_jobs").await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        helpers::pg_only(
            manager,
            "REVOKE UPDATE ON inference_jobs FROM yorishiro_app;",
        )
        .await
    }
}
