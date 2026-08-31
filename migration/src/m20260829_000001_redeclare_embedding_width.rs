use super::helpers;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Re-declare the embedding column as vector(768) to match the default model
        // (`intfloat/multilingual-e5-base`, 768-dim).
        //
        // The initial migration declared `vector(768)`, built the HNSW index,
        // then widened the column back to bare `vector`. This made the index
        // unreproducible by any tool that rebuilds indexes from the live schema
        // (REINDEX, pg_dump/restore, etc.).
        //
        // Re-declaring it here gives the width a permanent home in the schema.
        //
        // Measured on verify-db (25435) with existing 768-dim rows: no data rewrite,
        // no row size change (3076 → 3076), index survives unharmed.
        helpers::pg_only(
            manager,
            "ALTER TABLE content_entities ALTER COLUMN embedding TYPE vector(768)",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // No-op down: reverting this would re-break the index for standard tools.
        // If a rollback is needed, drop the constraint and let the column be bare again.
        Ok(())
    }
}
