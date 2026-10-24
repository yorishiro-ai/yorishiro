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
            .table(Alias::new("workspace_invites"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
            .col(ColumnDef::new(Alias::new("email")).text().not_null())
            .col(ColumnDef::new(Alias::new("role")).text().not_null())
            .col(
                ColumnDef::new(Alias::new("token_hash"))
                    .blob()
                    .not_null()
                    .unique_key(),
            )
            .col(helpers::ts_tz_col(manager, Alias::new("expires_at")).not_null())
            .col(helpers::ts_tz_col(manager, Alias::new("used_at")))
            .col(helpers::created_at(manager))
            .foreign_key(
                ForeignKey::create()
                    .name("fk_workspace_invites_tenant_id")
                    .from(Alias::new("workspace_invites"), Alias::new("tenant_id"))
                    .to(Alias::new("tenant_tenants"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .to_owned();

        helpers::create_table_with_checks(
            manager,
            "workspace_invites",
            table,
            &[(
                "workspace_invites_role_check",
                "role IN ('owner', 'admin', 'member', 'viewer')",
            )],
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("workspace_invites_tenant_id_idx")
                    .table(Alias::new("workspace_invites"))
                    .col(Alias::new("tenant_id"))
                    .to_owned(),
            )
            .await?;

        helpers::enable_rls_with_policy(
            manager,
            "workspace_invites",
            "tenant_isolation",
            "tenant_id",
            "app.current_tenant",
            false,
        )
        .await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("workspace_invites"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
