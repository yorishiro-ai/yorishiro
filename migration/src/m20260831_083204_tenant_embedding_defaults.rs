use super::helpers;
use sea_orm_migration::prelude::*;

/// Add tenant-level embedding defaults to support a three-tier inheritance chain:
/// system → tenant → workspace.
///
/// A tenant without its own assignment returns `NULL` for both columns,
/// so workspaces under it fall back to the deployment default.
/// A tenant with an explicit default makes that the fallback for all its workspaces
/// unless a workspace overrides it.

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        helpers::pg_only(
            manager,
            "ALTER TABLE identity_tenants \
             ADD COLUMN embedding_model TEXT, \
             ADD COLUMN embedding_dimensions INTEGER",
        )
        .await?;

        helpers::sqlite_only(
            manager,
            "ALTER TABLE identity_tenants ADD COLUMN embedding_model TEXT; \
             ALTER TABLE identity_tenants ADD COLUMN embedding_dimensions INTEGER",
        )
        .await?;

        // Clear the "unconfigured" sentinel: existing databases have `identity_workspaces.embedding_model = 'unconfigured'`
        // from when no embedding provider was configured at creation time.
        // Leaving these rows would cause write-time model checks to fail for every subsequent embed.
        // Reset them to NULL so the first real embed stamps the correct model.
        helpers::pg_only(
            manager,
            "UPDATE identity_workspaces \
             SET embedding_model = NULL, embedding_dimensions = NULL \
             WHERE embedding_model = 'unconfigured'",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
