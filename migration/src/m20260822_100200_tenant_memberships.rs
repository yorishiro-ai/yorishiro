use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_tenant_memberships"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk())
                    .col(ColumnDef::new(Alias::new("tenant_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("user_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("role")).text().not_null())
                    .col(helpers::created_at())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_tenant_memberships_tenant_id")
                            .from(
                                Alias::new("identity_tenant_memberships"),
                                Alias::new("tenant_id"),
                            )
                            .to(Alias::new("identity_tenants"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_tenant_memberships_user_id")
                            .from(
                                Alias::new("identity_tenant_memberships"),
                                Alias::new("user_id"),
                            )
                            .to(Alias::new("identity_users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        let db = manager.get_connection();

        // UNIQUE (tenant_id, user_id): sea_query's Table::create() has no multi-column
        // unique-key builder that also composes cleanly with the two foreign_key() calls above,
        // so this is added as a separate index rather than a column modifier.
        manager
            .create_index(
                Index::create()
                    .name("uq_tenant_memberships_tenant_id_user_id")
                    .table(Alias::new("identity_tenant_memberships"))
                    .col(Alias::new("tenant_id"))
                    .col(Alias::new("user_id"))
                    .unique()
                    .to_owned(),
            )
            .await?;

        // role CHECK (role IN ('owner', 'admin', 'member', 'viewer')), added as a raw ALTER TABLE rather than sea_query's table-level .check() to stay a plain, easily-greppable statement.
        db.execute_unprepared(
            "ALTER TABLE identity_tenant_memberships \
             ADD CONSTRAINT tenant_memberships_role_check \
             CHECK (role IN ('owner', 'admin', 'member', 'viewer'));",
        )
        .await?;

        // Strict policy: current_setting('app.current_tenant')::uuid with no `true` (lenient)
        // argument, so a missing setting raises rather than matching nothing.
        helpers::enable_rls_with_policy(
            db,
            "identity_tenant_memberships",
            "tenant_isolation",
            "tenant_id",
            "app.current_tenant",
            false,
        )
        .await?;

        // No GRANT: identity_tenant_memberships is control-plane data, reached only through the migration-role identity pool (see src/db.rs), never a tenant-scoped request connection, same reasoning as identity_tenants and identity_users.
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("identity_tenant_memberships"))
                    .to_owned(),
            )
            .await
    }
}
