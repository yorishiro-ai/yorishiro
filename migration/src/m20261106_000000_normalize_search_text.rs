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
        helpers::pg_only(manager, "DROP INDEX entities_data_trgm_idx; CREATE INDEX entities_data_trgm_idx ON entity_entities USING gin (normalize(data::text, nfkc) gin_trgm_ops)").await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        helpers::pg_only(manager, "DROP INDEX entities_data_trgm_idx; CREATE INDEX entities_data_trgm_idx ON entity_entities USING gin ((data::text) gin_trgm_ops)").await
    }
}
