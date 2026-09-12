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
        let [created_at, updated_at] = helpers::timestamps(manager);

        let table = Table::create()
            .table(Alias::new("compute_credit_ledger"))
            .if_not_exists()
            .col(uuidv7_pk(manager))
            .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
            .col(
                ColumnDef::new(Alias::new("amount"))
                    .big_integer()
                    .not_null(),
            )
            .col(
                ColumnDef::new(Alias::new("transaction_type"))
                    .text()
                    .not_null(),
            )
            .col(created_at)
            .col(updated_at)
            .foreign_key(
                ForeignKey::create()
                    .name("fk_compute_credit_ledger_workspace_id")
                    .from(
                        Alias::new("compute_credit_ledger"),
                        Alias::new("workspace_id"),
                    )
                    .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .to_owned();

        helpers::create_table_with_checks(
            manager,
            "compute_credit_ledger",
            table,
            &[(
                "compute_credit_ledger_transaction_type_check",
                "transaction_type IN ('earn', 'spend')",
            )],
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("compute_credit_ledger_workspace_id_created_at_idx")
                    .table(Alias::new("compute_credit_ledger"))
                    .col(Alias::new("workspace_id"))
                    .col((
                        Alias::new("created_at"),
                        sea_orm_migration::sea_orm::sea_query::IndexOrder::Desc,
                    ))
                    .to_owned(),
            )
            .await?;

        helpers::grant(manager, "SELECT, INSERT", "compute_credit_ledger").await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("compute_credit_ledger"))
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

fn uuidv7_pk(manager: &SchemaManager<'_>) -> ColumnDef {
    let mut col = ColumnDef::new(Alias::new("id"));
    if manager.get_database_backend() == sea_orm_migration::sea_orm::DbBackend::Sqlite {
        col.blob().not_null().primary_key();
    } else {
        col.uuid()
            .not_null()
            .primary_key()
            .default(Expr::cust("uuidv7()"));
    }
    col.to_owned()
}
