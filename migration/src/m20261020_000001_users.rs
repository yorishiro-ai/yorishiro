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
        let table = Table::create()
            .table(Alias::new("user_users"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(
                ColumnDef::new(Alias::new("email"))
                    .text()
                    .not_null()
                    .unique_key(),
            )
            .col(ColumnDef::new(Alias::new("password_hash")).text())
            .col(ColumnDef::new(Alias::new("display_name")).text())
            .col(ColumnDef::new(Alias::new("oauth_provider")).text())
            .col(ColumnDef::new(Alias::new("oauth_subject_id")).text())
            .col(helpers::created_at(manager))
            .to_owned();

        helpers::create_table_with_checks(
        manager,
        "user_users",
        table,
        &[(
            "user_users_auth_method_check",
            "(password_hash IS NOT NULL AND oauth_provider IS NULL AND oauth_subject_id IS NULL) \
                     OR (oauth_provider IS NOT NULL AND oauth_subject_id IS NOT NULL)",
        )],
    )
    .await?;

        manager
            .create_index(
                Index::create()
                    .name("user_users_oauth_identity_idx")
                    .unique()
                    .table(Alias::new("user_users"))
                    .col(Alias::new("oauth_provider"))
                    .col(Alias::new("oauth_subject_id"))
                    .and_where(Expr::col(Alias::new("oauth_provider")).is_not_null())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Alias::new("user_users")).to_owned())
            .await?;

        Ok(())
    }
}
