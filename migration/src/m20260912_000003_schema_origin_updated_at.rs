//! Add `origin_updated_at` to `schema_schemas` so the server can stamp a
//! push-notification flag when a template's definition changes: setting it to
//! `NULL` marks every following schema as having an upstream update available,
//! and the merge endpoint clears it on success.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("schema_schemas"))
                    .add_column(
                        ColumnDef::new(Alias::new("origin_updated_at")).timestamp_with_time_zone(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("schema_schemas"))
                    .drop_column(Alias::new("origin_updated_at"))
                    .to_owned(),
            )
            .await
    }
}
