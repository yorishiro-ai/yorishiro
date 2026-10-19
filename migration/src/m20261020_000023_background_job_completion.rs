//! Completion records commit atomically with bulk worker effects.

use sea_orm_migration::prelude::*;

use super::{helpers, m20260912_000005_fill_defaults_audit_action::replace_check};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        replace_check(manager, "action IN ('undo_migration_job', 'set_maintenance', 'reindex_embeddings', 'fill_defaults', 'background_job_completed')").await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Refuse rollback when completion records exist rather than deleting an audit trail.
        replace_check(manager, "action IN ('undo_migration_job', 'set_maintenance', 'reindex_embeddings', 'fill_defaults')").await
    }
}
