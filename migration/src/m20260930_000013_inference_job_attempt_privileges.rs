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
        helpers::enable_rls_with_policy(
            manager,
            "inference_jobs",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            false,
        )
        .await?;
        helpers::grant(manager, "SELECT, UPDATE", "inference_jobs").await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        helpers::pg_only(
            manager,
            "REVOKE SELECT, UPDATE ON inference_jobs FROM yorishiro_app;
             DROP POLICY IF EXISTS workspace_isolation ON inference_jobs;
             ALTER TABLE inference_jobs DISABLE ROW LEVEL SECURITY;",
        )
        .await
    }
}
