use super::helpers;
use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DbBackend;

/// The trigger body both backends share, differing only in how each spells "now".
/// One string with the timestamp expression substituted, so the two versions cannot drift in the part that matters: the `WHERE` clause deciding which rows detach.
fn detach_body(now: &str) -> String {
    format!(
        "UPDATE content_schemas \
            SET origin_status = 'detached', updated_at = {now} \
          WHERE origin_template_id = OLD.id \
            AND origin_status = 'linked';"
    )
}

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The whole schema, as one migration.
///
/// A fresh database applies this single file and gets the complete schema in one pass.
/// **This is not backward compatible and is not meant to be.** A database that applied a different migration history cannot apply this file: the version names do not match, and nothing here checks for or migrates from that state. Such a database is recreated, not upgraded.
///
/// Two things from the old file list are absent rather than merged, because merging them would have written statements that were immediately overwritten:
/// the pair of `authenticate_api_key` overloads that used to be created without `audit` and recreated with it in a later file, and nothing else.
///
/// Everything that looks like a later patch is still in its original position rather than folded into the table it patches.
/// That is deliberate for the two Postgres-only fixes at the end: `identity_workspace_worker_classes`'s CHECK and `identity_templates.created_by`'s `ON DELETE SET NULL` are absent on SQLite by design, and folding them into the shared `CREATE TABLE` would hand that backend constraints it is documented as not having.
/// The `ALTER TABLE` form is what keeps the two backends' end states different in the way they are supposed to be different.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // identity_tenants
        // Idempotent (`duplicate_object` swallowed): every `GRANT ... TO yorishiro_app` in a later migration, and every RLS-scoped connection's `after_connect` (`src/db.rs`), requires this role to already exist.
        // PostgreSQL 16+ doesn't let even the role's creator `SET ROLE` to it automatically, so the migration also grants membership to itself right after creating it.
        // A superuser migration role masks a missing grant here (it can `SET ROLE` regardless of membership), so verify this on a non-superuser role.
        //
        // No-op on SQLite: roles don't exist there, and a single-file database is its own tenant boundary.
        helpers::pg_only(
            manager,
            "DO $$ BEGIN \
             CREATE ROLE yorishiro_app NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION \
             NOBYPASSRLS NOLOGIN; \
             EXCEPTION WHEN duplicate_object THEN NULL; END $$; \
             GRANT yorishiro_app TO CURRENT_USER;",
        )
        .await?;

        // Tenant-level embedding defaults support the three-tier inheritance chain:
        // system → tenant → workspace.  A tenant without its own assignment returns
        // NULL for both columns, so workspaces under it fall back to the deployment
        // default.  A tenant with an explicit default makes that the fallback for all
        // its workspaces unless a workspace overrides it.
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_tenants"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(ColumnDef::new(Alias::new("name")).text().not_null())
                    .col(ColumnDef::new(Alias::new("max_workspaces")).integer())
                    .col(helpers::created_at())
                    .col(ColumnDef::new(Alias::new("embedding_model")).text())
                    .col(ColumnDef::new(Alias::new("embedding_dimensions")).integer())
                    .to_owned(),
            )
            .await?;

        // yorishiro_app gets no GRANT at all here: this table is reached only through the migration-role identity pool (see src/db.rs), never a tenant-scoped request connection.
        // Enabling RLS anyway is defense in depth against a future grant added without re-deriving this reasoning.
        helpers::enable_rls_with_policy(
            manager,
            "identity_tenants",
            "tenant_isolation",
            "id",
            "app.current_tenant",
            false,
        )
        .await?;

        // identity_users
        // password_hash is nullable: a user may be OAuth-provisioned instead of password-based.
        let table = Table::create()
            .table(Alias::new("identity_users"))
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
            .col(helpers::created_at())
            .to_owned();

        // Every row is either password-authenticated (password_hash set, oauth_* both NULL) or OAuth-provisioned (oauth_provider + oauth_subject_id set, password_hash may be NULL), never a mix and never neither.
        helpers::create_table_with_checks(
            manager,
            "identity_users",
            table,
            &[(
                "users_auth_method_check",
                "(password_hash IS NOT NULL AND oauth_provider IS NULL AND oauth_subject_id IS NULL) \
                 OR (oauth_provider IS NOT NULL AND oauth_subject_id IS NOT NULL)",
            )],
        )
        .await?;

        // The subject id ("sub" claim) is only unique within its provider, so the uniqueness key is the pair, not either column alone.
        // Partial (WHERE oauth_provider IS NOT NULL) so password-only rows, which leave both columns NULL, never collide on the unique index.
        // SQLite supports partial indexes with the same WHERE syntax, so this builder path produces valid SQL on both backends.
        manager
            .create_index(
                Index::create()
                    .name("users_oauth_identity_idx")
                    .unique()
                    .table(Alias::new("identity_users"))
                    .col(Alias::new("oauth_provider"))
                    .col(Alias::new("oauth_subject_id"))
                    .and_where(Expr::col(Alias::new("oauth_provider")).is_not_null())
                    .to_owned(),
            )
            .await?;

        // No RLS and no GRANT for this table: identity_users is a control-plane table reached only through the migration-role identity pool (see src/db.rs), never a tenant-scoped request connection, so yorishiro_app has no business touching it directly.

        // identity_tenant_memberships
        let table = Table::create()
            .table(Alias::new("identity_tenant_memberships"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
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
            .to_owned();

        // role CHECK (role IN ('owner', 'admin', 'member', 'viewer')).
        helpers::create_table_with_checks(
            manager,
            "identity_tenant_memberships",
            table,
            &[(
                "tenant_memberships_role_check",
                "role IN ('owner', 'admin', 'member', 'viewer')",
            )],
        )
        .await?;

        // UNIQUE (tenant_id, user_id): sea_query's Table::create() has no multi-column unique-key builder that also composes cleanly with the two foreign_key() calls above, so this is added as a separate index rather than a column modifier.
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

        // Strict policy: current_setting('app.current_tenant')::uuid with no `true` (lenient) argument, so a missing setting raises rather than matching nothing.
        helpers::enable_rls_with_policy(
            manager,
            "identity_tenant_memberships",
            "tenant_isolation",
            "tenant_id",
            "app.current_tenant",
            false,
        )
        .await?;

        // No GRANT: identity_tenant_memberships is control-plane data, reached only through the migration-role identity pool (see src/db.rs), never a tenant-scoped request connection, same reasoning as identity_tenants and identity_users.

        // identity_workspaces
        let mut table = Table::create()
            .table(Alias::new("identity_workspaces"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(ColumnDef::new(Alias::new("tenant_id")).uuid().not_null())
            .col(ColumnDef::new(Alias::new("name")).text().not_null())
            .col(ColumnDef::new(Alias::new("max_entities")).integer())
            // A workspace exists before its schema does: `admin create-workspace` leaves it pending, and creating the schema marks it active.
            .col(
                ColumnDef::new(Alias::new("status"))
                    .text()
                    .not_null()
                    .default("schema_pending"),
            )
            // The model a workspace's vectors were produced by, and their width.
            // NULL means the deployment default, recorded so a workspace whose model changed can be told from one provisioned under a different one.
            .col(ColumnDef::new(Alias::new("embedding_model")).text())
            .col(ColumnDef::new(Alias::new("embedding_dimensions")).integer())
            // Circular reference: a schema also names its workspace.
            // No foreign_key constraint here on Postgres since content_schemas may not exist yet when this migration runs; that FK is added further down, once that table exists.
            .col(ColumnDef::new(Alias::new("schema_id")).uuid())
            .col(helpers::created_at())
            .foreign_key(
                ForeignKey::create()
                    .name("fk_identity_workspaces_tenant_id")
                    .from(Alias::new("identity_workspaces"), Alias::new("tenant_id"))
                    .to(Alias::new("identity_tenants"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .to_owned();

        // SQLite doesn't resolve FK targets at DDL time (no CREATE SCHEMA/dependency ordering to fight), so the schema_id FK can go in here directly instead of waiting for a later ALTER TABLE like Postgres does.
        if manager.get_database_backend() == DbBackend::Sqlite {
            table = table
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_identity_workspaces_schema_id")
                        .from(Alias::new("identity_workspaces"), Alias::new("schema_id"))
                        .to(Alias::new("content_schemas"), Alias::new("id")),
                )
                .to_owned();
        }

        helpers::create_table_with_checks(
            manager,
            "identity_workspaces",
            table,
            &[
                (
                    "identity_workspaces_status_check",
                    "status IN ('schema_pending', 'active')",
                ),
                (
                    "identity_workspaces_embedding_dimensions_check",
                    "embedding_dimensions IS NULL OR embedding_dimensions > 0",
                ),
            ],
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("identity_workspaces_tenant_id_name_key")
                    .table(Alias::new("identity_workspaces"))
                    .col(Alias::new("tenant_id"))
                    .col(Alias::new("name"))
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Strict form: `yorishiro_app` sets both GUCs on every connection, so reaching this table without a tenant is a bug, and raising surfaces it rather than reading as an empty tenant.
        helpers::enable_rls_with_policy(
            manager,
            "identity_workspaces",
            "tenant_isolation",
            "tenant_id",
            "app.current_tenant",
            false,
        )
        .await?;

        helpers::grant(manager, "SELECT", "identity_workspaces").await?;

        // Column-level GRANT UPDATE: `status` and `schema_id` are the whole of what a request writes here.
        // `max_entities`, `name` and the embedding stamp are provisioning decisions; a request that could rewrite its own quota is a different system.
        // Issued raw since helpers::grant()'s signature only expresses whole-table grants.
        helpers::pg_only(
            manager,
            "GRANT UPDATE (status, schema_id) ON identity_workspaces TO yorishiro_app;",
        )
        .await?;

        // identity_api_keys
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_api_keys"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
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

        // RLS policy is an OR of two lenient conditions, not the single-column equality helpers::enable_rls_with_policy expresses, so it is written directly here.
        //
        // A workspace-scoped key is visible when its workspace_id matches the session's.
        // A tenant-scoped key (workspace_id NULL) is instead visible to any session in its tenant, since NULL = <uuid> is NULL rather than true and would otherwise make such a key's own row invisible to the very session authenticated by it.
        //
        // Both reads use NULLIF(current_setting(..., true), '') rather than the strict form: this table is also reached by the control-plane pool, where neither app.current_workspace nor app.current_tenant is set, and an unguarded read there would fail the query outright rather than matching no rows.
        //
        // No-op on SQLite: a single-tenant, single-file database has no other tenant/workspace's keys to hide.
        helpers::pg_only(
            manager,
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

        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "identity_api_keys",
        )
        .await?;

        // identity_invites
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

        // identity_templates
        let [created_at, updated_at] = helpers::timestamps();
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_templates"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(ColumnDef::new(Alias::new("tenant_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("name")).text().not_null())
                    .col(ColumnDef::new(Alias::new("description")).text())
                    .col(
                        ColumnDef::new(Alias::new("definition"))
                            .json_binary()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("locale")).text())
                    .col(
                        ColumnDef::new(Alias::new("visibility"))
                            .text()
                            .not_null()
                            .default("tenant"),
                    )
                    .col(ColumnDef::new(Alias::new("author")).text())
                    .col(ColumnDef::new(Alias::new("fork_of")).uuid())
                    .col(ColumnDef::new(Alias::new("created_by")).uuid())
                    .col(created_at)
                    .col(updated_at)
                    .check(Expr::cust("visibility IN ('tenant', 'community')"))
                    .index(
                        Index::create()
                            .name("templates_tenant_id_name_key")
                            .unique()
                            .col(Alias::new("tenant_id"))
                            .col(Alias::new("name")),
                    )
                    // No ON DELETE action on this FK, unlike every other tenant_id FK in this schema: RESTRICT/NO ACTION is the default, so deleting a tenant with templates fails loudly instead of cascading them away.
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_templates_tenant_id")
                            .from(Alias::new("identity_templates"), Alias::new("tenant_id"))
                            .to(Alias::new("identity_tenants"), Alias::new("id")),
                    )
                    // Self-referential: deleting a template others were forked from must leave the forks usable, losing only the pointer back.
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_templates_fork_of")
                            .from(Alias::new("identity_templates"), Alias::new("fork_of"))
                            .to(Alias::new("identity_templates"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_templates_created_by")
                            .from(Alias::new("identity_templates"), Alias::new("created_by"))
                            .to(Alias::new("identity_users"), Alias::new("id")),
                    )
                    .to_owned(),
            )
            .await?;

        // sea_query's ColumnDef has no TEXT[] helper, so this column is added as raw SQL right after create_table.
        // SQLite has no array type at all: the equivalent column there holds the same tag list JSON-encoded (a TEXT column of a JSON array), read/written as such by the application on that backend rather than as a native Postgres array.
        helpers::pg_only(
            manager,
            "ALTER TABLE identity_templates ADD COLUMN tags TEXT[] NOT NULL DEFAULT '{}';",
        )
        .await?;
        helpers::sqlite_only(
            manager,
            "ALTER TABLE identity_templates ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';",
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("templates_tenant_id_idx")
                    .table(Alias::new("identity_templates"))
                    .col(Alias::new("tenant_id"))
                    .to_owned(),
            )
            .await?;

        // GIN index over a TEXT[] column: create_index has no operator-class expression for this, so it goes through raw SQL.
        // No SQLite equivalent: `tags` is plain JSON-encoded TEXT there, not an indexable array type.
        helpers::pg_only(
            manager,
            "CREATE INDEX templates_tags_idx ON identity_templates USING gin(tags);",
        )
        .await?;

        // No RLS on this table, deliberately: template queries run as the owner through the repository layer, which scopes by tenant in the query itself, because a policy would have to read the table the app role holds no grant on, and a policy the role cannot evaluate fails the query rather than filtering it.
        //
        // No GRANT either, for the same reason: yorishiro_app is never the role that reaches this table, so there is nothing to grant it.

        // identity_maintenance
        let table = Table::create()
            .table(Alias::new("identity_maintenance"))
            .if_not_exists()
            // One row, enforced by the primary key.
            // Not the usual uuidv7_pk() shape: the PK is a boolean singleton, checked below to be exactly TRUE.
            .col(
                ColumnDef::new(Alias::new("id"))
                    .boolean()
                    .not_null()
                    .primary_key()
                    .default(true),
            )
            .col(
                ColumnDef::new(Alias::new("mode"))
                    .text()
                    .not_null()
                    .default("off"),
            )
            .col(
                ColumnDef::new(Alias::new("retry_after"))
                    .integer()
                    .not_null()
                    .default(300),
            )
            .col(ColumnDef::new(Alias::new("reason")).text())
            // No `created_at`: this is a singleton row that exists from migration time, not a created record.
            .col(
                ColumnDef::new(Alias::new("updated_at"))
                    .timestamp_with_time_zone()
                    .not_null()
                    .default(Expr::current_timestamp()),
            )
            .to_owned();

        // CHECK (id): sea_query's ColumnDef has no boolean-must-be-true constraint, so this is a table-level constraint added right after create_table rather than a column modifier.
        // `mode` and `retry_after` CHECKs follow the same path for the same reason.
        helpers::create_table_with_checks(
            manager,
            "identity_maintenance",
            table,
            &[
                ("maintenance_id_check", "id"),
                (
                    "maintenance_mode_check",
                    "mode IN ('off', 'read_only', 'full_lock')",
                ),
                ("maintenance_retry_after_check", "retry_after > 0"),
            ],
        )
        .await?;

        // The single seed row: `read_only` sheds writes, `full_lock` sheds everything but the health probes, and this row is what every request checks to decide which applies.
        // The migration crate has no generated entity types, so this uses raw SQL.
        manager
            .get_connection()
            .execute_unprepared("INSERT INTO identity_maintenance (id) VALUES (TRUE);")
            .await?;

        // No RLS on this table: it is a global singleton, not tenant- or workspace-scoped data.
        //
        // GRANT is SELECT only: yorishiro_app reads the maintenance flag on every request but never writes it (writes go through the migration-role/admin path instead).
        helpers::grant(manager, "SELECT", "identity_maintenance").await?;

        // content_schemas
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

        // The circular half: `identity_workspaces.schema_id` is created as a plain column with no FK above, since content_schemas did not exist yet at that point.
        // content_schemas exists now, so the FK it deferred is added here.
        //
        // No-op on SQLite: that backend doesn't resolve FK targets at DDL time, so the FK is declared inline in its own CREATE TABLE instead of deferring it.
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

        // Partial index: SeaORM's `and_where` builder produces the same WHERE clause on both backends.
        manager
            .create_index(
                Index::create()
                    .name("schemas_origin_template_idx")
                    .table(Alias::new("content_schemas"))
                    .col(Alias::new("origin_template_id"))
                    .and_where(Expr::col(Alias::new("origin_template_id")).is_not_null())
                    .to_owned(),
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

        // content_entities
        let [created_at, updated_at] = helpers::timestamps();

        manager
            .create_table(
                Table::create()
                    .table(Alias::new("content_entities"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("schema_id")).uuid().not_null())
                    .col(
                        ColumnDef::new(Alias::new("schema_version"))
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("entity_type")).text().not_null())
                    .col(ColumnDef::new(Alias::new("data")).json_binary().not_null())
                    .col(ColumnDef::new(Alias::new("created_by")).uuid())
                    .col(ColumnDef::new(Alias::new("updated_by")).uuid())
                    .col(created_at)
                    .col(updated_at)
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_content_entities_workspace_id")
                            .from(Alias::new("content_entities"), Alias::new("workspace_id"))
                            .to(Alias::new("identity_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    // No ON DELETE action in the old DDL (line 239): default NO ACTION.
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_content_entities_schema_id")
                            .from(Alias::new("content_entities"), Alias::new("schema_id"))
                            .to(Alias::new("content_schemas"), Alias::new("id")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_content_entities_created_by")
                            .from(Alias::new("content_entities"), Alias::new("created_by"))
                            .to(Alias::new("identity_users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_content_entities_updated_by")
                            .from(Alias::new("content_entities"), Alias::new("updated_by"))
                            .to(Alias::new("identity_users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        // Embeddings live in a separate `content_entity_embeddings` table so that:
        // 1. SQLite can store vectors as raw LE f32 BLOBs (pgvector has no equivalent)
        // 2. `content_entities` has no `PgVector` column, so its read/write path is
        //    the same on both backends with no branching around column lists.
        //
        // Fresh database: content_entities has no `embedding` column at all.
        // content_entity_embeddings(entity_id UUID, embedding vector(768)) holds
        // the vectors, with an HNSW index for KNN search on PostgreSQL.
        helpers::pg_only(
            manager,
            "CREATE TABLE content_entity_embeddings (\
                 entity_id UUID PRIMARY KEY REFERENCES content_entities(id) ON DELETE CASCADE,\
                 embedding vector(768)\
             ); \
             CREATE INDEX entities_embedding_hnsw ON content_entity_embeddings \
             USING hnsw (embedding vector_cosine_ops)",
        )
        .await?;

        // SQLite path: `content_entity_embeddings` stores vectors as opaque BLOBs.
        // entity_id is the join key (not implicit rowid): VACUUM may renumber
        // implicit rowids, so we must never join on them (see sqlite.org/lang_vacuum.html).
        // KNN search uses plain table scan with `vec_distance_cosine` rather than
        // vec0 virtual tables — no MATCH/k= syntax needed.
        // SQLite never had an `embedding` column on `content_entities` — vectors are
        // stored exclusively in this table.
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("content_entity_embeddings"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("entity_id"))
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("embedding")).blob())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_entity_embeddings_entity_id")
                            .from(
                                Alias::new("content_entity_embeddings"),
                                Alias::new("entity_id"),
                            )
                            .to(Alias::new("content_entities"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // The multi-column composite is expressible through `create_index`; the GIN/trigram
        // indexes that follow are not.
        manager
            .create_index(
                Index::create()
                    .name("entities_workspace_type_idx")
                    .table(Alias::new("content_entities"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("entity_type"))
                    .col(Alias::new("created_at"))
                    .to_owned(),
            )
            .await?;

        // GIN/trigram indexes: Postgres-only (pgvector, pg_trgm), no-ops on SQLite.
        helpers::pg_only(
            manager,
            "CREATE INDEX entities_data_gin ON content_entities USING GIN (data jsonb_path_ops)",
        )
        .await?;
        helpers::pg_only(
            manager,
            "CREATE INDEX entities_data_trgm_idx ON content_entities USING gin ((data::text) gin_trgm_ops)",
        )
        .await?;

        // FTS5 virtual table for text search fallback on SQLite.
        // SQLite has no pg_trgm, so full-text search uses FTS5 with stored content:
        // the triggers below explicitly INSERT/DELETE rows in the FTS5 table,
        // so the virtual table doesn't need a `content=` mapping to a backing table.
        // This avoids the problem that `content_entities` has no `entity_id` column
        // (vectors live in `content_entity_embeddings`), while still storing the
        // UUID join key in the FTS5 index for `e.id = fts.entity_id` lookups.
        helpers::sqlite_only(
            manager,
            "CREATE VIRTUAL TABLE fts_content_entities USING fts5(\
                data,\
                workspace_id,\
                entity_id UNINDEXED,\
                content=''\
            )",
        )
        .await?;
        // Triggers keep the FTS5 virtual table in sync with the backing table.
        // `entity_id` is written as `NEW.id` / `OLD.id` (not rowid) so the FTS5
        // join in `search.rs` (`e.id = fts.entity_id`) works regardless of VACUUM
        // or any other renumbering. The `rowid` is still needed by FTS5 internally.
        helpers::sqlite_only(
            manager,
            "CREATE TRIGGER fts_content_entities_insert AFTER INSERT ON content_entities \
             BEGIN \
                INSERT INTO fts_content_entities(rowid, data, workspace_id, entity_id) \
                VALUES(NEW.id, NEW.data, NEW.workspace_id, NEW.id); \
             END",
        )
        .await?;
        helpers::sqlite_only(
            manager,
            "CREATE TRIGGER fts_content_entities_update AFTER UPDATE ON content_entities \
             BEGIN \
                INSERT INTO fts_content_entities(fts_content_entities, rowid, data, workspace_id, entity_id) \
                VALUES('delete', OLD.id, OLD.data, OLD.workspace_id, OLD.id); \
                INSERT INTO fts_content_entities(rowid, data, workspace_id, entity_id) \
                VALUES(NEW.id, NEW.data, NEW.workspace_id, NEW.id); \
             END",
        )
        .await?;
        helpers::sqlite_only(
            manager,
            "CREATE TRIGGER fts_content_entities_delete AFTER DELETE ON content_entities \
             BEGIN \
                INSERT INTO fts_content_entities(fts_content_entities, rowid, data, workspace_id, entity_id) \
                VALUES('delete', OLD.id, OLD.data, OLD.workspace_id, OLD.id); \
             END",
        )
        .await?;

        // Strict form, on purpose: yorishiro_app sets both app.current_tenant and app.current_workspace, so reaching this table without a workspace set is a bug.
        // Raising surfaces that bug; a lenient policy would instead read it as an empty workspace and hide it.
        helpers::enable_rls_with_policy(
            manager,
            "content_entities",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            false,
        )
        .await?;

        // Every table is granted individually: all tables share one schema, so a
        // wildcard grant would sweep in tables that must stay ungranted.
        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "content_entities",
        )
        .await?;

        // content_relations
        let table = Table::create()
            .table(Alias::new("content_relations"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
            .col(ColumnDef::new(Alias::new("source_id")).uuid().not_null())
            .col(ColumnDef::new(Alias::new("target_id")).uuid().not_null())
            .col(
                ColumnDef::new(Alias::new("relation_type"))
                    .text()
                    .not_null(),
            )
            .col(
                ColumnDef::new(Alias::new("properties"))
                    .json_binary()
                    .not_null()
                    .default(Expr::cust("'{}'")),
            )
            .col(
                ColumnDef::new(Alias::new("status"))
                    .text()
                    .not_null()
                    .default("active"),
            )
            .col(helpers::created_at())
            .foreign_key(
                ForeignKey::create()
                    .name("fk_content_relations_workspace_id")
                    .from(Alias::new("content_relations"), Alias::new("workspace_id"))
                    .to(Alias::new("identity_workspaces"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_content_relations_source_id")
                    .from(Alias::new("content_relations"), Alias::new("source_id"))
                    .to(Alias::new("content_entities"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_content_relations_target_id")
                    .from(Alias::new("content_relations"), Alias::new("target_id"))
                    .to(Alias::new("content_entities"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .index(
                Index::create()
                    .name("relations_unique")
                    .unique()
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("source_id"))
                    .col(Alias::new("target_id"))
                    .col(Alias::new("relation_type")),
            )
            .to_owned();

        // sea_query's Table::create() check() support was not reached for; a raw
        // ALTER TABLE ... ADD CONSTRAINT is simpler and matches the old DDL's inline CHECK exactly.
        helpers::create_table_with_checks(
            manager,
            "content_relations",
            table,
            &[(
                "content_relations_status_check",
                "status IN ('active', 'deprecated', 'archived')",
            )],
        )
        .await?;

        // `status` is in both indexes because every traversal filters on it.
        // Three-column composite indexes: SeaORM builder expresses these directly.
        manager
            .create_index(
                Index::create()
                    .name("relations_source_idx")
                    .table(Alias::new("content_relations"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("source_id"))
                    .col(Alias::new("status"))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("relations_target_idx")
                    .table(Alias::new("content_relations"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("target_id"))
                    .col(Alias::new("status"))
                    .to_owned(),
            )
            .await?;

        // Strict, not lenient: `yorishiro_app` sets both GUCs on every connection, so reaching this table without a workspace set is a bug, and raising surfaces it where matching zero rows would look like an empty workspace.
        helpers::enable_rls_with_policy(
            manager,
            "content_relations",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            false,
        )
        .await?;

        // Individualized per helpers::grant()'s own rule against wildcard grants now that every table shares one schema.
        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "content_relations",
        )
        .await?;

        // content_entity_snapshots
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("content_entity_snapshots"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    // Batch job identifier, not a row reference: no REFERENCES clause in the old DDL, so no foreign key here either.
                    .col(ColumnDef::new(Alias::new("job_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
                    // The entity a snapshot was taken of.
                    // No foreign key: an entity can be deleted while a snapshot of it remains, per the old DDL having no REFERENCES clause on this column.
                    .col(ColumnDef::new(Alias::new("entity_id")).uuid().not_null())
                    // No foreign key in the old DDL either.
                    .col(ColumnDef::new(Alias::new("schema_id")).uuid().not_null())
                    .col(
                        ColumnDef::new(Alias::new("schema_version"))
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("data")).json_binary().not_null())
                    .col(helpers::created_at())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_content_entity_snapshots_workspace_id")
                            .from(
                                Alias::new("content_entity_snapshots"),
                                Alias::new("workspace_id"),
                            )
                            .to(Alias::new("identity_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("entity_snapshots_job_idx")
                    .table(Alias::new("content_entity_snapshots"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("job_id"))
                    .to_owned(),
            )
            .await?;

        // DESC ordering on created_at: sea_query's Index::create supports per-column sort direction.
        manager
            .create_index(
                Index::create()
                    .name("entity_snapshots_entity_idx")
                    .table(Alias::new("content_entity_snapshots"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("entity_id"))
                    .col((Alias::new("created_at"), IndexOrder::Desc))
                    .to_owned(),
            )
            .await?;

        // Lenient: grouped with content.schemas and content.fill_proposals in the old DDL's strict/lenient split, since the control-plane pool also reaches this table over a connection that has not named a workspace, and must match nothing rather than raise.
        helpers::enable_rls_with_policy(
            manager,
            "content_entity_snapshots",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            true,
        )
        .await?;

        // The old DDL granted this table via a schema-wide "GRANT ... ON ALL TABLES IN SCHEMA content", now individualized per-table since every table shares one schema after the public-schema unification.
        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "content_entity_snapshots",
        )
        .await?;

        // identity_tenant_billing
        let [created_at, updated_at] = helpers::timestamps();
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_tenant_billing"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("tenant_id"))
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("plan")).text())
                    .col(
                        ColumnDef::new(Alias::new("stripe_customer_id"))
                            .text()
                            .unique_key(),
                    )
                    .col(created_at)
                    .col(updated_at)
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_tenant_billing_tenant_id")
                            .from(
                                Alias::new("identity_tenant_billing"),
                                Alias::new("tenant_id"),
                            )
                            .to(Alias::new("identity_tenants"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // No GRANT and RLS anyway, for the reason identity_tenants gives above: this table is reached only through ctx.db, never a tenant-scoped request connection.
        helpers::enable_rls_with_policy(
            manager,
            "identity_tenant_billing",
            "tenant_billing_isolation",
            "tenant_id",
            "app.current_tenant",
            false,
        )
        .await?;

        // identity_stripe_processed_events
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_stripe_processed_events"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("event_id"))
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("event_type")).text().not_null())
                    .col(ColumnDef::new(Alias::new("customer_id")).text())
                    .col(
                        ColumnDef::new(Alias::new("stripe_created"))
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(helpers::created_at())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("stripe_processed_events_customer_id_idx")
                    .table(Alias::new("identity_stripe_processed_events"))
                    .col(Alias::new("customer_id"))
                    .col(Alias::new("stripe_created"))
                    .and_where(Expr::col(Alias::new("customer_id")).is_not_null())
                    .to_owned(),
            )
            .await?;

        // yorishiro_app gets no GRANT at all here, matching identity_tenants/identity_tenant_billing: this table is reached only through ctx.db (the migration-role connection, the Stripe webhook handler's only DB access), never a tenant-scoped request connection.
        // No RLS either: unlike tenant_billing this table has no tenant_id column to scope a policy on (it's keyed by Stripe's own event id), and it is never reached by anything but ctx.db regardless.

        // identity_template_versions
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_template_versions"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(ColumnDef::new(Alias::new("template_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("version")).integer().not_null())
                    .col(
                        ColumnDef::new(Alias::new("definition"))
                            .json_binary()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("changelog")).text())
                    // draft: visible only to the owning tenant.
                    // pre: published but announced as unstable.
                    // stable: the version an installer gets by default.
                    .col(
                        ColumnDef::new(Alias::new("status"))
                            .text()
                            .not_null()
                            .default("draft"),
                    )
                    .col(ColumnDef::new(Alias::new("created_by")).uuid())
                    .col(helpers::created_at())
                    .check(Expr::cust("status IN ('draft', 'pre', 'stable')"))
                    .index(
                        Index::create()
                            .name("template_versions_template_id_version_key")
                            .unique()
                            .col(Alias::new("template_id"))
                            .col(Alias::new("version")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_template_versions_template_id")
                            .from(
                                Alias::new("identity_template_versions"),
                                Alias::new("template_id"),
                            )
                            .to(Alias::new("identity_templates"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_template_versions_created_by")
                            .from(
                                Alias::new("identity_template_versions"),
                                Alias::new("created_by"),
                            )
                            .to(Alias::new("identity_users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("template_versions_template_idx")
                    .table(Alias::new("identity_template_versions"))
                    .col(Alias::new("template_id"))
                    .col((Alias::new("version"), IndexOrder::Desc))
                    .to_owned(),
            )
            .await?;

        // One review per tenant per template: a tenant that used a template twice does not get two votes, and updating an opinion is an UPDATE (upsert_review's ON CONFLICT) rather than a second row.
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_template_reviews"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(ColumnDef::new(Alias::new("template_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("tenant_id")).uuid().not_null())
                    .col(
                        ColumnDef::new(Alias::new("rating"))
                            .small_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("comment")).text())
                    .col(ColumnDef::new(Alias::new("created_by")).uuid())
                    .col(helpers::created_at())
                    .col(
                        ColumnDef::new(Alias::new("updated_at"))
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .check(Expr::cust("rating BETWEEN 1 AND 5"))
                    .index(
                        Index::create()
                            .name("template_reviews_template_id_tenant_id_key")
                            .unique()
                            .col(Alias::new("template_id"))
                            .col(Alias::new("tenant_id")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_template_reviews_template_id")
                            .from(
                                Alias::new("identity_template_reviews"),
                                Alias::new("template_id"),
                            )
                            .to(Alias::new("identity_templates"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_template_reviews_tenant_id")
                            .from(
                                Alias::new("identity_template_reviews"),
                                Alias::new("tenant_id"),
                            )
                            .to(Alias::new("identity_tenants"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_template_reviews_created_by")
                            .from(
                                Alias::new("identity_template_reviews"),
                                Alias::new("created_by"),
                            )
                            .to(Alias::new("identity_users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("template_reviews_template_idx")
                    .table(Alias::new("identity_template_reviews"))
                    .col(Alias::new("template_id"))
                    .to_owned(),
            )
            .await?;

        // No RLS, no GRANT to yorishiro_app on either table, matching identity_templates itself: both are read in exactly the same paths (the repository layer, as the owner role, scoping by tenant in the query) and are never reached through the tenant pool.

        // identity_workspace_llm_keys
        let [created_at, updated_at] = helpers::timestamps();
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_workspace_llm_keys"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("workspace_id"))
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("base_url")).text().not_null())
                    .col(ColumnDef::new(Alias::new("model")).text().not_null())
                    .col(ColumnDef::new(Alias::new("api_key")).text().not_null())
                    .col(created_at)
                    .col(updated_at)
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_workspace_llm_keys_workspace_id")
                            .from(
                                Alias::new("identity_workspace_llm_keys"),
                                Alias::new("workspace_id"),
                            )
                            .to(Alias::new("identity_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // No RLS and no GRANT, deliberately, matching identity_templates: yorishiro_app is never the role that reaches this table.
        // Reads and writes go through the migration-role pool (ctx.db), which is what keeps a workspace's credentials off the RLS-scoped request connection entirely rather than relying on a policy being right.

        // columns
        let [created_at, updated_at] = helpers::timestamps();
        let sqlite = manager.get_database_backend() == DbBackend::Sqlite;

        // Field names, in display order.
        // sea_query's ColumnDef has no default-expression-plus-json_binary combination that reads well here, so the DEFAULT goes on as a follow-up ALTER COLUMN on Postgres.
        // SQLite has no ALTER COLUMN at all (only rename/add/drop-column), so the default is set inline in the CREATE TABLE there instead; `'[]'::jsonb` is Postgres cast syntax with no SQLite equivalent, so the SQLite default is the bare JSON literal.
        let mut columns_col = ColumnDef::new(Alias::new("columns"))
            .json_binary()
            .not_null()
            .to_owned();
        if sqlite {
            columns_col.default(Expr::cust("'[]'"));
        }

        let table = Table::create()
            .table(Alias::new("content_entity_column_preferences"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
            .col(ColumnDef::new(Alias::new("entity_type")).text().not_null())
            .col(columns_col)
            .col(created_at)
            .col(updated_at)
            // The upsert target: without it, two tabs saving at once leave two rows and the reader picks one arbitrarily.
            .index(
                Index::create()
                    .name("entity_column_preferences_workspace_entity_type_key")
                    .unique()
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("entity_type")),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_content_entity_column_preferences_workspace_id")
                    .from(
                        Alias::new("content_entity_column_preferences"),
                        Alias::new("workspace_id"),
                    )
                    .to(Alias::new("identity_workspaces"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .to_owned();

        // `jsonb_typeof(columns) = 'array'` is Postgres's jsonb function; SQLite's JSON1 extension spells the same check `json_type(columns) = 'array'`, so the two backends get different check expressions under the same constraint name rather than one shared string.
        helpers::create_table_with_checks(
            manager,
            "content_entity_column_preferences",
            table,
            &[(
                "entity_column_preferences_columns_is_array",
                if sqlite {
                    "json_type(columns) = 'array'"
                } else {
                    "jsonb_typeof(columns) = 'array'"
                },
            )],
        )
        .await?;

        // No-op on SQLite: the default is already inline in the CREATE TABLE above.
        helpers::pg_only(
            manager,
            "ALTER TABLE content_entity_column_preferences \
             ALTER COLUMN columns SET DEFAULT '[]'::jsonb;",
        )
        .await?;

        // Lenient: the control-plane pool also reaches this table over a connection that has not named a workspace, and must match nothing rather than raise.
        helpers::enable_rls_with_policy(
            manager,
            "content_entity_column_preferences",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            true,
        )
        .await?;

        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "content_entity_column_preferences",
        )
        .await?;

        // identity_api_key_audit_log
        // Independent of `scope`, deliberately: `scope` stays the four-way ordered read/write/schema/migration ladder (`ApiKeyScope`'s derived `Ord`), and folding audit into that ladder above `migration` would let an audit-reading key also run a batch migration and flip maintenance mode, which nobody asked for.
        // A separate boolean composes with any scope instead: a key is `scope=read, audit=true` to read the log without write access, or any other scope plus audit, entirely independent of where that scope sits in the ladder.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE identity_api_keys ADD COLUMN audit boolean NOT NULL DEFAULT false;",
            )
            .await?;

        // identity_api_key_audit_log
        let table = Table::create()
            .table(Alias::new("identity_api_key_audit_log"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
            .col(ColumnDef::new(Alias::new("tenant_id")).uuid().not_null())
            // No foreign_key(): identity_api_keys::revoke deletes the key row outright (see its own doc comment), and a hard FK, even ON DELETE SET NULL, would still require the referenced row to exist at insert time and would touch this table on every revoke.
            // Recorded as a plain UUID so "which key" survives the key's own deletion; a caller resolves it against identity_api_keys and reads a miss as "since revoked".
            .col(ColumnDef::new(Alias::new("api_key_id")).uuid())
            .col(ColumnDef::new(Alias::new("user_id")).uuid())
            // A closed set, matching the as_db_str()/from_db_str() pattern every other stored-string enum in this codebase uses (MaintenanceMode, MembershipRole, ApiKeyScope): the CHECK constraint below is what stops a typo'd action from silently becoming a fourth value nothing filters on.
            .col(ColumnDef::new(Alias::new("action")).text().not_null())
            // Free-form context for the action (e.g. the migration job id an undo reverted, the maintenance mode a switch moved to and from).
            // Not part of the closed action set: this is detail a reader inspects, not something the database branches on.
            .col(
                ColumnDef::new(Alias::new("detail"))
                    .json_binary()
                    .not_null(),
            )
            .col(helpers::created_at())
            .foreign_key(
                ForeignKey::create()
                    .name("fk_identity_api_key_audit_log_workspace_id")
                    .from(
                        Alias::new("identity_api_key_audit_log"),
                        Alias::new("workspace_id"),
                    )
                    .to(Alias::new("identity_workspaces"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_identity_api_key_audit_log_tenant_id")
                    .from(
                        Alias::new("identity_api_key_audit_log"),
                        Alias::new("tenant_id"),
                    )
                    .to(Alias::new("identity_tenants"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            // SetNull, not the no-FK-at-all treatment api_key_id gets above: identity_users rows are not routinely deleted the way a revoked key is, so a real FK here is cheap, and SetNull keeps a record of "some now-deleted user did this" rather than losing the row's referential integrity.
            .foreign_key(
                ForeignKey::create()
                    .name("fk_identity_api_key_audit_log_user_id")
                    .from(
                        Alias::new("identity_api_key_audit_log"),
                        Alias::new("user_id"),
                    )
                    .to(Alias::new("identity_users"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::SetNull),
            )
            .to_owned();

        helpers::create_table_with_checks(
            manager,
            "identity_api_key_audit_log",
            table,
            &[(
                "identity_api_key_audit_log_action_check",
                "action IN ('undo_migration_job', 'set_maintenance', 'reindex_embeddings')",
            )],
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("api_key_audit_log_workspace_id_created_at_idx")
                    .table(Alias::new("identity_api_key_audit_log"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("created_at"))
                    .to_owned(),
            )
            .await?;

        // Strict form, matching content_entities: yorishiro_app sets both app.current_tenant and app.current_workspace on every connection, so a write reaching this table without a workspace set is a bug, and raising surfaces that bug rather than silently writing a row nobody can read back.
        // The one write path that runs on ctx.db instead (set_maintenance, a deployment-wide operation) supplies the acting key's own tenant_id/workspace_id explicitly as column values rather than relying on the connection's GUCs, since ctx.db is the migration-role connection and carries neither.
        //
        // FORCE ROW LEVEL SECURITY is deliberately not applied: it would also bind the table owner (the migration role that performs the set_maintenance audit insert on ctx.db), and a superuser migration role bypasses FORCE regardless, so it would add nothing under test-loco's roles while silently breaking the ctx.db write path on a deployment whose migration role is a non-superuser owner.
        helpers::enable_rls_with_policy(
            manager,
            "identity_api_key_audit_log",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            false,
        )
        .await?;

        // SELECT, INSERT only, deliberately, never UPDATE/DELETE: an audit trail a key can rewrite or erase isn't one.
        // yorishiro_app can append new rows and read them back, but has no way to alter or remove what has already landed.
        helpers::grant(manager, "SELECT, INSERT", "identity_api_key_audit_log").await?;

        // Both overloads, with `audit` in the returned column list.
        // `RETURNS TABLE`'s column list cannot be widened with ALTER FUNCTION, so both
        // overloads are created in their final shape from the start.
        // `authenticate` (services::auth) needs `audit` to populate `AuthContext`, which is what `require_audit` reads to decide whether a key may reach the audit-log read endpoint.
        //
        // No-op on SQLite: no stored function exists on that backend, and `authenticate_sqlite` is the entity-API replica that stands in for it.
        helpers::pg_only(
            manager,
            "CREATE FUNCTION authenticate_api_key(p_key_hash bytea)
             RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text, user_id uuid, audit boolean)
             LANGUAGE sql
             SECURITY DEFINER
             SET search_path = pg_catalog, public
             AS $$
               SELECT k.id, k.workspace_id, w.tenant_id, k.scope, k.user_id, k.audit
               FROM identity_api_keys k
               JOIN identity_workspaces w ON w.id = k.workspace_id
               WHERE k.key_hash = p_key_hash
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
             SET search_path = pg_catalog, public
             AS $$
               SELECT k.id,
                      COALESCE(k.workspace_id, w.id) AS workspace_id,
                      k.tenant_id,
                      k.scope,
                      k.user_id,
                      k.audit
               FROM identity_api_keys k
               LEFT JOIN identity_workspaces w
                      ON k.workspace_id IS NULL
                     AND w.id = p_requested_workspace
                     AND w.tenant_id = k.tenant_id
               WHERE k.key_hash = p_key_hash
                 AND (k.workspace_id IS NOT NULL OR w.id IS NOT NULL)
             $$;

             REVOKE ALL ON FUNCTION authenticate_api_key(bytea, uuid) FROM PUBLIC;
             GRANT EXECUTE ON FUNCTION authenticate_api_key(bytea, uuid) TO yorishiro_app;",
        )
        .await?;

        // identity_workspace_embedding_keys
        let [created_at, updated_at] = helpers::timestamps();
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_workspace_embedding_keys"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("workspace_id"))
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("base_url")).text().not_null())
                    .col(ColumnDef::new(Alias::new("model")).text().not_null())
                    .col(ColumnDef::new(Alias::new("api_key")).text().not_null())
                    .col(
                        ColumnDef::new(Alias::new("dimensions"))
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("send_dimensions_param"))
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(created_at)
                    .col(updated_at)
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_workspace_embedding_keys_workspace_id")
                            .from(
                                Alias::new("identity_workspace_embedding_keys"),
                                Alias::new("workspace_id"),
                            )
                            .to(Alias::new("identity_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // No RLS and no GRANT, deliberately, matching identity_workspace_llm_keys: yorishiro_app is never the role that reaches this table.
        // Reads and writes go through the migration-role pool (ctx.db), which keeps a workspace's embedding credentials off the RLS-scoped request connection entirely rather than relying on a policy being right.

        // identity_workspace_worker_classes
        let [created_at, updated_at] = helpers::timestamps();
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_workspace_worker_classes"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("workspace_id"))
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    // The same three-value string WorkerClass::as_db_str/from_db_str already round-trip
                    // (base's own serde(rename_all = "snake_case") wire form), so a row read here and a
                    // value read off a queued job's payload are byte-identical.
                    .col(ColumnDef::new(Alias::new("worker_class")).text().not_null())
                    .col(created_at)
                    .col(updated_at)
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_workspace_worker_classes_workspace_id")
                            .from(
                                Alias::new("identity_workspace_worker_classes"),
                                Alias::new("workspace_id"),
                            )
                            .to(Alias::new("identity_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // No RLS and no GRANT, deliberately, matching identity_workspace_embedding_keys/
        // identity_workspace_llm_keys: yorishiro_app never reaches this table. Reads and writes go
        // through the migration-role pool (ctx.db), keeping which compute a workspace's jobs run on
        // off the RLS-scoped request connection entirely rather than relying on a policy being right.

        // 1. `identity_workspace_worker_classes.worker_class` accepted any string.
        //
        // Its siblings carry this constraint (`identity_tenant_memberships.role`, `identity_api_keys.scope`, the audit log's `action`), for the reason `action`'s own migration states: a CHECK is what stops a typo'd value from silently becoming a fourth thing nothing filters on.
        // A serde round-trip guarantees only that *this* writer emits a valid value: a manual UPDATE, a data-fix script, or a future code path reaching the column directly are all outside that guarantee, and this column decides which worker process dequeues a job.
        // The three values are `WorkerClass::as_db_str`'s own output, so a variant added there without adding it here fails closed at the database rather than routing jobs to a queue nothing reads.
        //
        // **PostgreSQL only, and SQLite is left without this constraint.** SQLite's `ALTER TABLE` cannot add one to an existing table, and the only way to gain it there is to rebuild the table and copy every row.
        // That is a large, risky migration to close a gap on the tier documented as "trying Yorishiro out or personal use" (`docs/sqlite.md`), so it is not done here: on SQLite this column still accepts any string, and a deployment that needs the constraint needs PostgreSQL.
        helpers::pg_only(
            manager,
            "ALTER TABLE identity_workspace_worker_classes \
             ADD CONSTRAINT identity_workspace_worker_classes_worker_class_check \
             CHECK (worker_class IN ('tenant_private', 'official', 'shared'))",
        )
        .await?;

        // 2. `identity_templates.created_by` defaults to NO ON DELETE, which would prevent deleting a user who authored a template.
        //
        // SET NULL, matching `fork_of` on this same table: the column is nullable, a template outlives the account that wrote it, and the alternatives are both wrong here.
        // CASCADE would delete a tenant's templates because an author closed their account, destroying data belonging to the tenant rather than to the user.
        // RESTRICT requires deleting or re-authoring every template a user ever wrote before the user can be deleted.
        // Losing the authorship attribution is the acceptable half of that trade; losing the template is not.
        //
        // **PostgreSQL only.** `identity_templates` exists on SQLite too: it is created unconditionally above, and the `pg_only`/`sqlite_only` calls cover the `tags` column's type and a GIN index, not the table itself.
        // SQLite cannot alter a foreign key's action in place, so `created_by` keeps NO ACTION there and deleting a user who authored a template still fails with `FOREIGN KEY constraint failed` (measured directly against a SQLite file, not inferred).
        helpers::pg_only(
            manager,
            "ALTER TABLE identity_templates \
             DROP CONSTRAINT fk_identity_templates_created_by",
        )
        .await?;
        helpers::pg_only(
            manager,
            "ALTER TABLE identity_templates \
             ADD CONSTRAINT fk_identity_templates_created_by \
             FOREIGN KEY (created_by) REFERENCES identity_users(id) ON DELETE SET NULL",
        )
        .await?;

        // 3. `content_schemas` had no `updated_at`, while every other mutable table here does.
        //
        // The table is not append-only: the `detach_orphaned_schema_origin` trigger created alongside it rewrites `origin_status` in place when an upstream template is deleted, so a row could change with nothing recording when.
        //
        // Added through the schema builder rather than raw SQL because this table exists on SQLite too, and that backend's `ALTER TABLE` cannot do `ALTER COLUMN ... SET NOT NULL`.
        // Nullable for the same reason, and because SQLite refuses a non-constant default on `ADD COLUMN` outright (measured: `Cannot add a column with non-constant default`), so an existing table cannot be given a `now()` default there.
        //
        // `None` therefore means exactly one thing: the row has not been written by any path since this column was added.
        // Every write path stamps it from here on (both triggers below, `content_schemas::create_schema`, and that module's archival `update_many`), so `None` is purely historical and its population only shrinks; it is not a state a new or modified row can enter.
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("content_schemas"))
                    .add_column(
                        ColumnDef::new(Alias::new("updated_at"))
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // PostgreSQL can carry a default; SQLite cannot, per the measurement above.
        // Which is why the stamping is not left to the column on either backend: the guarantee has to hold where the weaker engine is, so every write path stamps explicitly and this default is belt-and-braces for Postgres rather than the mechanism.
        helpers::pg_only(
            manager,
            "ALTER TABLE content_schemas ALTER COLUMN updated_at SET DEFAULT now()",
        )
        .await?;

        // The triggers are recreated so they stamp `updated_at` on the same in-place rewrite this column exists to record.
        // A trigger that detaches a schema without stamping `updated_at` would leave the column recording every change except the one its own justification names.
        helpers::pg_only(
            manager,
            &format!(
                "CREATE OR REPLACE FUNCTION detach_orphaned_schema_origin() RETURNS TRIGGER AS $$
                 BEGIN
                   {}
                   RETURN OLD;
                 END;
                 $$ LANGUAGE plpgsql SECURITY DEFINER;",
                detach_body("now()")
            ),
        )
        .await?;

        // SQLite has no CREATE OR REPLACE for triggers, so the existing one is dropped first.
        // `AFTER DELETE` rather than `BEFORE`: SQLite's `OLD` is valid inside the trigger body either way, and `AFTER` avoids racing the row's own deletion.
        helpers::sqlite_only(
            manager,
            "DROP TRIGGER IF EXISTS templates_detach_schema_origins",
        )
        .await?;
        helpers::sqlite_only(
            manager,
            &format!(
                "CREATE TRIGGER templates_detach_schema_origins
                 AFTER DELETE ON identity_templates
                 FOR EACH ROW
                 BEGIN
                   {}
                 END;",
                detach_body("CURRENT_TIMESTAMP")
            ),
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Everything this migration created, in an order that never drops a table another still references.
        // Triggers and policies go with their tables; the two functions and the role do not, so they are named.
        helpers::pg_only(
            manager,
            "DROP FUNCTION IF EXISTS authenticate_api_key(bytea); \
             DROP FUNCTION IF EXISTS authenticate_api_key(bytea, uuid); \
             DROP FUNCTION IF EXISTS detach_orphaned_schema_origin() CASCADE;",
        )
        .await?;

        // `identity_workspaces.schema_id` references `content_schemas`, which references
        // `identity_workspaces` back: the circularity `up()` breaks by adding this one
        // constraint after both tables exist. Dropping in dependency order cannot break it,
        // because neither table can go first, so the constraint is removed before the loop
        // rather than left for the table drop to trip over.
        //
        // Postgres only, matching `up()`: SQLite declares this FK inline in the table itself,
        // so there is no separate constraint to drop and the table drop carries it away.
        helpers::pg_only(
            manager,
            "ALTER TABLE identity_workspaces \
             DROP CONSTRAINT IF EXISTS fk_identity_workspaces_schema_id",
        )
        .await?;

        for table in [
            "content_relations",
            "content_entity_snapshots",
            "content_entity_column_preferences",
            "fts_content_entities",
            "content_entities",
            "content_schemas",
            "identity_api_key_audit_log",
            "identity_api_keys",
            "identity_invites",
            "identity_template_reviews",
            "identity_template_versions",
            "identity_templates",
            "identity_workspace_worker_classes",
            "identity_workspace_embedding_keys",
            "identity_workspace_llm_keys",
            "identity_stripe_processed_events",
            "identity_tenant_billing",
            "identity_maintenance",
            "identity_workspaces",
            "identity_tenant_memberships",
            "identity_users",
            "identity_tenants",
        ] {
            manager
                .drop_table(
                    Table::drop()
                        .table(Alias::new(table))
                        .if_exists()
                        .to_owned(),
                )
                .await?;
        }

        // The role outlives the schema on purpose: it is created idempotently by `up()`, other databases in the same cluster may still be using it, and dropping a role that owns objects elsewhere fails anyway.
        Ok(())
    }
}
