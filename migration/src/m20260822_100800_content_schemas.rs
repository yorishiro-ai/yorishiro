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
                    .table(Alias::new("content_schemas"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(ColumnDef::new(Alias::new("tenant_id")).uuid().not_null())
                    // Schemas are scoped to a workspace, not a tenant: each workspace holds its own copy of a template, and editing one must not reach its siblings.
                    // `tenant_id` stays for the cross-tenant reads (community-visible templates, export).
                    .col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("name")).text().not_null())
                    .col(
                        ColumnDef::new(Alias::new("version"))
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .col(
                        ColumnDef::new(Alias::new("definition"))
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("status"))
                            .text()
                            .not_null()
                            .default("active"),
                    )
                    // Where this schema came from, and whether it still follows it.
                    // A hand-written schema is `detached` and has never been linked, told apart from an orphan by `origin_template_id` having never been set.
                    // `origin_snapshot` is the definition as copied, which is what a three-way comparison needs as its base.
                    .col(ColumnDef::new(Alias::new("origin_template_id")).uuid())
                    .col(
                        ColumnDef::new(Alias::new("origin_status"))
                            .text()
                            .not_null()
                            .default("detached"),
                    )
                    .col(ColumnDef::new(Alias::new("origin_snapshot")).json_binary())
                    .col(helpers::created_at())
                    .check(Expr::cust("status IN ('active', 'archived')"))
                    .check(Expr::cust("origin_status IN ('linked', 'detached')"))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_content_schemas_tenant_id")
                            .from(Alias::new("content_schemas"), Alias::new("tenant_id"))
                            .to(Alias::new("identity_tenants"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_content_schemas_workspace_id")
                            .from(Alias::new("content_schemas"), Alias::new("workspace_id"))
                            .to(Alias::new("identity_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_content_schemas_origin_template_id")
                            .from(
                                Alias::new("content_schemas"),
                                Alias::new("origin_template_id"),
                            )
                            .to(Alias::new("identity_templates"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        // The circular half (old DDL lines 216-217): identity_workspaces.schema_id was added as a plain column with no FK by m20260822_110500_workspaces, since content_schemas did not exist yet at that point.
        // content_schemas exists now, so the FK it deferred is added here.
        //
        // No-op on SQLite: that backend doesn't resolve FK targets at DDL time, so m20260822_100300_workspaces already declared this FK inline in its own CREATE TABLE instead of deferring it.
        helpers::pg_only(
            manager,
            "ALTER TABLE identity_workspaces \
             ADD CONSTRAINT fk_identity_workspaces_schema_id \
             FOREIGN KEY (schema_id) REFERENCES content_schemas(id);",
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("schemas_workspace_name_version_key")
                    .table(Alias::new("content_schemas"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("name"))
                    .col(Alias::new("version"))
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("schemas_tenant_id_idx")
                    .table(Alias::new("content_schemas"))
                    .col(Alias::new("tenant_id"))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("schemas_workspace_id_idx")
                    .table(Alias::new("content_schemas"))
                    .col(Alias::new("workspace_id"))
                    .to_owned(),
            )
            .await?;

        // Partial index: create_index has no WHERE-clause support, so this uses raw SQL.
        // SQLite supports the same WHERE syntax on CREATE INDEX, so this runs unchanged on both backends.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX schemas_origin_template_idx \
             ON content_schemas (origin_template_id) \
             WHERE origin_template_id IS NOT NULL;",
            )
            .await?;

        // Deleting a template must not destroy the copies made from it, and must stop them claiming to follow something that is gone.
        // A trigger rather than application code, so a delete arriving from the admin CLI or a migration is covered too.
        //
        // Defined here (alongside content_schemas, the table it writes to) even though it fires on identity_templates, matching the old DDL's own ordering choice of placing it next to content.schemas rather than next to identity.templates.
        helpers::pg_only(
            manager,
            "CREATE FUNCTION detach_orphaned_schema_origin() RETURNS TRIGGER AS $$
             BEGIN
               UPDATE content_schemas
                  SET origin_status = 'detached'
                WHERE origin_template_id = OLD.id
                  AND origin_status = 'linked';
               RETURN OLD;
             END;
             $$ LANGUAGE plpgsql SECURITY DEFINER;",
        )
        .await?;

        helpers::pg_only(
            manager,
            "CREATE TRIGGER templates_detach_schema_origins
             BEFORE DELETE ON identity_templates
             FOR EACH ROW EXECUTE FUNCTION detach_orphaned_schema_origin();",
        )
        .await?;

        // SQLite has no separate CREATE FUNCTION/CREATE TRIGGER split, but it can express the same guarantee directly: an AFTER DELETE trigger (BEFORE would still see the row on both engines, but SQLite's OLD is only valid inside the trigger body regardless of timing, and AFTER avoids racing the row's own deletion) that detaches any schema whose origin_template_id pointed at the deleted template.
        helpers::sqlite_only(
            manager,
            "CREATE TRIGGER templates_detach_schema_origins
             AFTER DELETE ON identity_templates
             FOR EACH ROW
             BEGIN
               UPDATE content_schemas
                  SET origin_status = 'detached'
                WHERE origin_template_id = OLD.id
                  AND origin_status = 'linked';
             END;",
        )
        .await?;

        // Lenient: the control-plane pool reaches content_schemas over a connection that sets neither GUC, so a connection that has not named a workspace must match nothing instead of raising.
        // See the old DDL's comment on the strict/lenient split, which groups content.schemas with entity_snapshots and fill_proposals as the lenient set.
        helpers::enable_rls_with_policy(
            manager,
            "content_schemas",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            true,
        )
        .await?;

        // The old DDL granted this table via a schema-wide "GRANT ... ON ALL TABLES IN SCHEMA content", now individualized per-table since every table shares one schema after the public-schema unification.
        helpers::grant(manager, "SELECT, INSERT, UPDATE, DELETE", "content_schemas").await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        helpers::pg_only(
            manager,
            "ALTER TABLE identity_workspaces DROP CONSTRAINT IF EXISTS fk_identity_workspaces_schema_id;",
        )
        .await?;
        helpers::pg_only(
            manager,
            "DROP TRIGGER IF EXISTS templates_detach_schema_origins ON identity_templates;",
        )
        .await?;
        helpers::pg_only(
            manager,
            "DROP FUNCTION IF EXISTS detach_orphaned_schema_origin();",
        )
        .await?;
        helpers::sqlite_only(
            manager,
            "DROP TRIGGER IF EXISTS templates_detach_schema_origins;",
        )
        .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("content_schemas"))
                    .to_owned(),
            )
            .await
    }
}
