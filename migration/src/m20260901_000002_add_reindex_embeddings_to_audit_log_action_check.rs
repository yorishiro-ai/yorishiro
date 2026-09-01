use super::helpers;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        helpers::add_check_constraint(
            m,
            "identity_api_key_audit_log",
            "identity_api_key_audit_log_action_check",
            "action IN ('undo_migration_job', 'set_maintenance', 'reindex_embeddings')",
            true,
        )
        .await
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        helpers::add_check_constraint(
            m,
            "identity_api_key_audit_log",
            "identity_api_key_audit_log_action_check",
            "action IN ('undo_migration_job', 'set_maintenance')",
            true,
        )
        .await
    }
}
