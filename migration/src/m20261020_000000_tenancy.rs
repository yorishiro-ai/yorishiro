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
        up_tenants(manager).await?;
        up_users(manager).await?;
        up_tenant_memberships(manager).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        down_tenant_memberships(manager).await?;
        down_users(manager).await?;
        down_tenants(manager).await?;
        Ok(())
    }
}

async fn up_tenants(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    helpers::pg_only(
        manager,
        "DO $$ BEGIN \
                 CREATE ROLE yorishiro_app NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION \
                 NOBYPASSRLS NOLOGIN; \
                 EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$; \
                 GRANT yorishiro_app TO CURRENT_USER;",
    )
    .await?;

    manager
        .create_table(
            Table::create()
                .table(Alias::new("tenant_tenants"))
                .if_not_exists()
                .col(helpers::uuidv7_pk(manager))
                .col(ColumnDef::new(Alias::new("name")).text().not_null())
                .col(ColumnDef::new(Alias::new("max_workspaces")).integer())
                .col(helpers::created_at(manager))
                .col(helpers::ts_tz_col(manager, Alias::new("disabled_at")))
                .to_owned(),
        )
        .await?;

    helpers::enable_rls_with_policy(
        manager,
        "tenant_tenants",
        "tenant_isolation",
        "id",
        "app.current_tenant",
        false,
    )
    .await?;

    Ok(())
}

async fn down_tenants(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_table(Table::drop().table(Alias::new("tenant_tenants")).to_owned())
        .await?;

    Ok(())
}

async fn up_users(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
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

async fn down_users(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_table(Table::drop().table(Alias::new("user_users")).to_owned())
        .await?;

    Ok(())
}

async fn up_tenant_memberships(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
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

async fn down_tenant_memberships(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_table(
            Table::drop()
                .table(Alias::new("tenant_memberships"))
                .to_owned(),
        )
        .await?;

    Ok(())
}
