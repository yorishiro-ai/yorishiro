use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let table = Table::create()
            .table(Alias::new("identity_invites"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(ColumnDef::new(Alias::new("tenant_id")).uuid().not_null())
            .col(ColumnDef::new(Alias::new("email")).text().not_null())
            .col(ColumnDef::new(Alias::new("role")).text().not_null())
            .col(
                ColumnDef::new(Alias::new("token_hash"))
                    .blob()
                    .not_null()
                    .unique_key(),
            )
            .col(
                ColumnDef::new(Alias::new("expires_at"))
                    .timestamp_with_time_zone()
                    .not_null(),
            )
            .col(ColumnDef::new(Alias::new("used_at")).timestamp_with_time_zone())
            .col(helpers::created_at())
            .foreign_key(
                ForeignKey::create()
                    .name("fk_identity_invites_tenant_id")
                    .from(Alias::new("identity_invites"), Alias::new("tenant_id"))
                    .to(Alias::new("identity_tenants"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .to_owned();

        helpers::create_table_with_checks(
            manager,
            "identity_invites",
            table,
            &[(
                "identity_invites_role_check",
                "role IN ('owner', 'admin', 'member', 'viewer')",
            )],
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("invites_tenant_id_idx")
                    .table(Alias::new("identity_invites"))
                    .col(Alias::new("tenant_id"))
                    .to_owned(),
            )
            .await?;

        // Strict policy: yorishiro_app sets app.current_tenant on every connection, so this table is reached only through the tenant-scoped identity pool, never a connection missing the GUC.
        helpers::enable_rls_with_policy(
            manager,
            "identity_invites",
            "tenant_isolation",
            "tenant_id",
            "app.current_tenant",
            false,
        )
        .await?;

        // No GRANT: identity_invites is control-plane, same as identity_tenants, identity_users and identity_tenant_memberships.
        // The admin CLI creates invites as the schema owner during provisioning, before any request-scoped role needs to touch this table.

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("identity_invites"))
                    .to_owned(),
            )
            .await
    }
}
