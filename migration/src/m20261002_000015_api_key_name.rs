//! Add the operator-visible name used by the API-key lifecycle endpoints.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        crate::helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("api_keys"))
                    .add_column(
                        ColumnDef::new(Alias::new("name"))
                            .string()
                            .not_null()
                            .default("unnamed"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("api_keys"))
                    .drop_column(Alias::new("name"))
                    .to_owned(),
            )
            .await
    }
}
