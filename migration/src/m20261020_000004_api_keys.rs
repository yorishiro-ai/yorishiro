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
        up_api_keys(manager).await?;
        up_api_key_audit_log(manager).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        down_api_key_audit_log(manager).await?;
        down_api_keys(manager).await?;
        Ok(())
    }
}

async fn up_api_keys(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(Alias::new("api_keys"))
                .if_not_exists()
                .col(helpers::uuidv7_pk(manager))
                .col(
                    ColumnDef::new(Alias::new("name"))
                        .string()
                        .not_null()
                        .default("unnamed"),
                )
                .col(helpers::uuid_col(manager, Alias::new("workspace_id")))
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_api_keys_workspace_id")
                        .from(Alias::new("api_keys"), Alias::new("workspace_id"))
                        .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_api_keys_tenant_id")
                        .from(Alias::new("api_keys"), Alias::new("tenant_id"))
                        .to(Alias::new("tenant_tenants"), Alias::new("id"))
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .col(helpers::uuid_col(manager, Alias::new("user_id")))
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_api_keys_user_id")
                        .from(Alias::new("api_keys"), Alias::new("user_id"))
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
                .col(ColumnDef::new(Alias::new("scope")).text().not_null().check(
                    Expr::col(Alias::new("scope")).is_in(["read", "write", "schema", "migration"]),
                ))
                .col(helpers::created_at(manager))
                .col(helpers::ts_tz_col(manager, Alias::new("last_used_at")))
                .col(helpers::ts_tz_col(manager, Alias::new("expires_at")))
                .col(helpers::ts_tz_col(manager, Alias::new("revoked_at")))
                .col(helpers::uuid_col(manager, Alias::new("replaced_by_id")))
                .col(
                    ColumnDef::new(Alias::new("audit"))
                        .boolean()
                        .not_null()
                        .default(false),
                )
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("api_keys_tenant_id_idx")
                .table(Alias::new("api_keys"))
                .col(Alias::new("tenant_id"))
                .to_owned(),
        )
        .await?;

    helpers::pg_only(
        manager,
        "ALTER TABLE api_keys ENABLE ROW LEVEL SECURITY;
                 CREATE POLICY workspace_isolation ON api_keys
                   USING (
                     workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid
                     OR (
                       workspace_id IS NULL
                       AND tenant_id = NULLIF(current_setting('app.current_tenant', true), '')::uuid
                     )
                   );",
    )
    .await?;

    helpers::grant(manager, "SELECT, INSERT, UPDATE, DELETE", "api_keys").await?; // Both overloads, with `audit` in the returned column list.
    helpers::pg_only(
                manager,
                "CREATE FUNCTION authenticate_api_key(p_key_hash bytea)
                 RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text, user_id uuid, audit boolean)
                 LANGUAGE sql
                 SECURITY DEFINER
                 SET search_path = pg_catalog, public, pg_temp
                 AS $$
                    SELECT k.id, k.workspace_id, k.tenant_id, k.scope, k.user_id, k.audit
                   FROM api_keys k
                    LEFT JOIN workspace_workspaces w ON w.id = k.workspace_id
                    WHERE k.key_hash = p_key_hash AND k.revoked_at IS NULL
                      AND (k.expires_at IS NULL OR k.expires_at > CURRENT_TIMESTAMP)
                      AND (k.workspace_id IS NULL OR w.tenant_id = k.tenant_id)
                 $$;

                 REVOKE ALL ON FUNCTION authenticate_api_key(bytea) FROM PUBLIC;
                 GRANT EXECUTE ON FUNCTION authenticate_api_key(bytea) TO yorishiro_app;

                 CREATE FUNCTION authenticate_api_key(
                   p_key_hash bytea,
                   p_requested_workspace uuid
                 )
                 RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text, user_id uuid, audit boolean)
                 LANGUAGE sql
                 SECURITY DEFINER
                 SET search_path = pg_catalog, public, pg_temp
                 AS $$
                   SELECT k.id,
                          COALESCE(k.workspace_id, w.id) AS workspace_id,
                          k.tenant_id,
                          k.scope,
                          k.user_id,
                          k.audit
                   FROM api_keys k
                   LEFT JOIN workspace_workspaces w
                          ON k.workspace_id IS NULL
                         AND w.id = p_requested_workspace
                         AND w.tenant_id = k.tenant_id
                    WHERE k.key_hash = p_key_hash AND k.revoked_at IS NULL
                      AND (k.expires_at IS NULL OR k.expires_at > CURRENT_TIMESTAMP)
                     AND (k.workspace_id IS NOT NULL OR w.id IS NOT NULL)
                 $$;

                 REVOKE ALL ON FUNCTION authenticate_api_key(bytea, uuid) FROM PUBLIC;
                 GRANT EXECUTE ON FUNCTION authenticate_api_key(bytea, uuid) TO yorishiro_app;",
            )
            .await?;

    Ok(())
}

async fn down_api_keys(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    helpers::pg_only(manager,"DROP FUNCTION authenticate_api_key(bytea); DROP FUNCTION authenticate_api_key(bytea, uuid);").await?;
    manager
        .drop_table(Table::drop().table(Alias::new("api_keys")).to_owned())
        .await?;

    Ok(())
}

async fn up_api_key_audit_log(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let table = Table::create()
        .table(Alias::new("api_key_audit_log"))
        .if_not_exists()
        .col(helpers::uuidv7_pk(manager))
        .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
        .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
        .col(helpers::uuid_col(manager, Alias::new("api_key_id")))
        .col(helpers::uuid_col(manager, Alias::new("user_id")))
        .col(ColumnDef::new(Alias::new("action")).text().not_null())
        .col(helpers::json_col(manager, Alias::new("detail")).not_null())
        .col(helpers::created_at(manager))
        .foreign_key(
            ForeignKey::create()
                .name("fk_api_key_audit_log_workspace_id")
                .from(Alias::new("api_key_audit_log"), Alias::new("workspace_id"))
                .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .name("fk_api_key_audit_log_tenant_id")
                .from(Alias::new("api_key_audit_log"), Alias::new("tenant_id"))
                .to(Alias::new("tenant_tenants"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .name("fk_api_key_audit_log_user_id")
                .from(Alias::new("api_key_audit_log"), Alias::new("user_id"))
                .to(Alias::new("user_users"), Alias::new("id"))
                .on_delete(ForeignKeyAction::SetNull),
        )
        .to_owned();

    helpers::create_table_with_checks(
        manager,
        "api_key_audit_log",
        table,
        &[(
            "api_key_audit_log_action_check",
            "action IN ('undo_migration_job', 'set_maintenance', 'reindex_embeddings', 'fill_defaults', 'background_job_completed')",
        )],
    )
    .await?;

    manager
        .create_index(
            Index::create()
                .name("api_key_audit_log_workspace_id_created_at_idx")
                .table(Alias::new("api_key_audit_log"))
                .col(Alias::new("workspace_id"))
                .col(Alias::new("created_at"))
                .to_owned(),
        )
        .await?;

    helpers::enable_rls_with_policy(
        manager,
        "api_key_audit_log",
        "workspace_isolation",
        "workspace_id",
        "app.current_workspace",
        false,
    )
    .await?;

    helpers::grant(manager, "SELECT, INSERT", "api_key_audit_log").await?;

    Ok(())
}

async fn down_api_key_audit_log(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_table(
            Table::drop()
                .table(Alias::new("api_key_audit_log"))
                .to_owned(),
        )
        .await?;

    Ok(())
}
