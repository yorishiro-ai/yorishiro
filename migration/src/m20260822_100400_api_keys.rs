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
                    .table(Alias::new("identity_api_keys"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk())
                    // Nullable: a tenant-scoped key (one that spans every workspace in its tenant) carries no workspace_id at all.
                    .col(ColumnDef::new(Alias::new("workspace_id")).uuid())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_keys_workspace_id")
                            .from(Alias::new("identity_api_keys"), Alias::new("workspace_id"))
                            .to(Alias::new("identity_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .col(ColumnDef::new(Alias::new("tenant_id")).uuid().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_keys_tenant_id")
                            .from(Alias::new("identity_api_keys"), Alias::new("tenant_id"))
                            .to(Alias::new("identity_tenants"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .col(ColumnDef::new(Alias::new("user_id")).uuid())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_keys_user_id")
                            .from(Alias::new("identity_api_keys"), Alias::new("user_id"))
                            .to(Alias::new("identity_users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .col(
                        ColumnDef::new(Alias::new("key_hash"))
                            .binary()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Alias::new("key_prefix")).text().not_null())
                    // `migration` ranks above `schema`: registering a schema adds a version nothing has been written against yet, while a batch migration rewrites stored rows.
                    .col(
                        ColumnDef::new(Alias::new("scope"))
                            .text()
                            .not_null()
                            .check(Expr::cust(
                                "scope IN ('read', 'write', 'schema', 'migration')",
                            )),
                    )
                    .col(helpers::created_at())
                    .col(ColumnDef::new(Alias::new("last_used_at")).timestamp_with_time_zone())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("api_keys_tenant_id_idx")
                    .table(Alias::new("identity_api_keys"))
                    .col(Alias::new("tenant_id"))
                    .to_owned(),
            )
            .await?;

        let db = manager.get_connection();

        // RLS policy is an OR of two lenient conditions, not the single-column equality helpers::enable_rls_with_policy expresses, so it is written directly here.
        //
        // A workspace-scoped key is visible when its workspace_id matches the session's.
        // A tenant-scoped key (workspace_id NULL) is instead visible to any session in its tenant, since NULL = <uuid> is NULL rather than true and would otherwise make such a key's own row invisible to the very session authenticated by it.
        //
        // Both reads use NULLIF(current_setting(..., true), '') rather than the strict form: this table is also reached by the control-plane pool, where neither app.current_workspace nor app.current_tenant is set, and an unguarded read there would fail the query outright rather than matching no rows.
        db.execute_unprepared(
            "ALTER TABLE identity_api_keys ENABLE ROW LEVEL SECURITY;
             CREATE POLICY workspace_isolation ON identity_api_keys
               USING (
                 workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid
                 OR (
                   workspace_id IS NULL
                   AND tenant_id = NULLIF(current_setting('app.current_tenant', true), '')::uuid
                 )
               );",
        )
        .await?;

        helpers::grant(db, "SELECT, INSERT, UPDATE, DELETE", "identity_api_keys").await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("identity_api_keys"))
                    .to_owned(),
            )
            .await
    }
}
