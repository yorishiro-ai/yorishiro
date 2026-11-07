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
            .table(Alias::new("tenant_memberships"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
            .col(helpers::uuid_col(manager, Alias::new("user_id")).not_null())
            .col(ColumnDef::new(Alias::new("role")).text().not_null())
            .col(helpers::created_at(manager))
            .foreign_key(
                ForeignKey::create()
                    .name("fk_tenant_memberships_tenant_id")
                    .from(Alias::new("tenant_memberships"), Alias::new("tenant_id"))
                    .to(Alias::new("tenant_tenants"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_tenant_memberships_user_id")
                    .from(Alias::new("tenant_memberships"), Alias::new("user_id"))
                    .to(Alias::new("user_users"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .to_owned();

        helpers::create_table_with_checks(
            manager,
            "tenant_memberships",
            table,
            &[(
                "tenant_memberships_role_check",
                "role IN ('owner', 'admin', 'member', 'viewer')",
            )],
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_tenant_memberships_tenant_id_user_id")
                    .table(Alias::new("tenant_memberships"))
                    .col(Alias::new("tenant_id"))
                    .col(Alias::new("user_id"))
                    .unique()
                    .to_owned(),
            )
            .await?;

        helpers::enable_rls_with_policy(
            manager,
            "tenant_memberships",
            "tenant_isolation",
            "tenant_id",
            "app.current_tenant",
            false,
        )
        .await?;

        manager
            .create_index(
                sea_orm_migration::sea_query::Index::create()
                    .name("idx_tenant_memberships_user_id")
                    .table(Alias::new("tenant_memberships"))
                    .col(Alias::new("user_id"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("tenant_memberships"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
