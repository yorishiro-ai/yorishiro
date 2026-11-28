use super::helpers;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        Box::pin(up_access_keys(manager)).await?;
        Box::pin(up_job_results(manager)).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        Box::pin(down_job_results(manager)).await?;
        Box::pin(down_access_keys(manager)).await?;
        Ok(())
    }
}

async fn up_access_keys(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(Alias::new("access_keys"))
                .col(helpers::uuidv7_pk(manager))
                .col(
                    ColumnDef::new(Alias::new("name"))
                        .string()
                        .not_null()
                        .default("unnamed"),
                )
                .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_access_keys_tenant_id")
                        .from(Alias::new("access_keys"), Alias::new("tenant_id"))
                        .to(Alias::new("tenant_tenants"), Alias::new("id"))
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .col(helpers::uuid_col(manager, Alias::new("user_id")))
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_access_keys_user_id")
                        .from(Alias::new("access_keys"), Alias::new("user_id"))
                        .to(Alias::new("user_users"), Alias::new("id"))
                        .on_delete(ForeignKeyAction::SetNull),
                )
                .col(
                    ColumnDef::new(Alias::new("key_hash"))
                        .binary()
                        .not_null()
                        .unique_key(),
                )
                .col(ColumnDef::new(Alias::new("key_prefix")).text().not_null())
                .col(helpers::created_at(manager))
                .col(helpers::ts_tz_col(manager, Alias::new("last_used_at")))
                .col(helpers::ts_tz_col(manager, Alias::new("expires_at")))
                .col(helpers::ts_tz_col(manager, Alias::new("revoked_at")))
                .col(
                    ColumnDef::new(Alias::new("enabled"))
                        .boolean()
                        .not_null()
                        .default(true),
                )
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("access_keys_tenant_id_idx")
                .table(Alias::new("access_keys"))
                .col(Alias::new("tenant_id"))
                .to_owned(),
        )
        .await?;

    helpers::pg_only(
        manager,
        "ALTER TABLE access_keys ENABLE ROW LEVEL SECURITY;
                 CREATE POLICY tenant_isolation ON access_keys
                   USING (tenant_id = NULLIF(current_setting('app.current_tenant', true), '')::uuid);",
    )
    .await?;

    helpers::grant(manager, "SELECT, INSERT, UPDATE, DELETE", "access_keys").await?;
    helpers::pg_only(
        manager,
        "CREATE FUNCTION authenticate_access_key(p_key_hash bytea)
                 RETURNS TABLE (id uuid, tenant_id uuid, user_id uuid)
                 LANGUAGE sql
                 SECURITY DEFINER
                 SET search_path = pg_catalog, public, pg_temp
                 AS $$
                   SELECT k.id, k.tenant_id, k.user_id
                   FROM access_keys k
                   WHERE k.key_hash = p_key_hash AND k.revoked_at IS NULL AND k.enabled
                     AND (k.expires_at IS NULL OR k.expires_at > CURRENT_TIMESTAMP)
                 $$;

                 REVOKE ALL ON FUNCTION authenticate_access_key(bytea) FROM PUBLIC;
                 GRANT EXECUTE ON FUNCTION authenticate_access_key(bytea) TO yorishiro_app;",
    )
    .await?;

    Ok(())
}

async fn down_access_keys(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    helpers::pg_only(manager, "DROP FUNCTION authenticate_access_key(bytea);").await?;
    manager
        .drop_table(Table::drop().table(Alias::new("access_keys")).to_owned())
        .await?;

    Ok(())
}

async fn up_job_results(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let table = Table::create()
        .table(Alias::new("job_results"))
        .col(helpers::uuidv7_pk(manager))
        .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
        .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
        .col(helpers::uuid_col(manager, Alias::new("access_key_id")))
        .col(helpers::uuid_col(manager, Alias::new("user_id")))
        .col(ColumnDef::new(Alias::new("action")).text().not_null())
        .col(helpers::json_col(manager, Alias::new("detail")).not_null())
        .col(helpers::created_at(manager))
        .foreign_key(
            ForeignKey::create()
                .name("fk_job_results_workspace_id")
                .from(Alias::new("job_results"), Alias::new("workspace_id"))
                .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .name("fk_job_results_tenant_id")
                .from(Alias::new("job_results"), Alias::new("tenant_id"))
                .to(Alias::new("tenant_tenants"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .name("fk_job_results_user_id")
                .from(Alias::new("job_results"), Alias::new("user_id"))
                .to(Alias::new("user_users"), Alias::new("id"))
                .on_delete(ForeignKeyAction::SetNull),
        )
        .to_owned();

    helpers::create_table_with_checks(
        manager,
        "job_results",
        table,
        &[(
            "job_results_action_check",
            "action IN ('undo_migration_job', 'fill_defaults', 'background_job_completed')",
        )],
    )
    .await?;

    manager
        .create_index(
            Index::create()
                .name("job_results_workspace_id_created_at_idx")
                .table(Alias::new("job_results"))
                .col(Alias::new("workspace_id"))
                .col(Alias::new("created_at"))
                .to_owned(),
        )
        .await?;

    helpers::enable_rls_with_policy(
        manager,
        "job_results",
        "workspace_isolation",
        "workspace_id",
        "app.current_workspace",
        false,
    )
    .await?;

    helpers::grant(manager, "SELECT, INSERT", "job_results").await?;

    Ok(())
}

async fn down_job_results(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_table(Table::drop().table(Alias::new("job_results")).to_owned())
        .await?;

    Ok(())
}
