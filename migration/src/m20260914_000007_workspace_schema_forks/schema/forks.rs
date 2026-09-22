use sea_orm_migration::prelude::*;

use crate::helpers;

pub(super) async fn create(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let [created_at, updated_at] = helpers::timestamps(manager);
    let table = Table::create()
        .table(Alias::new("workspace_schema_forks"))
        .if_not_exists()
        .col(helpers::uuidv7_pk(manager))
        .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
        .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
        .col(helpers::uuid_col(manager, Alias::new("source_workspace_id")).not_null())
        .col(helpers::uuid_col(manager, Alias::new("source_schema_id")).not_null())
        .col(
            ColumnDef::new(Alias::new("source_schema_version"))
                .integer()
                .not_null(),
        )
        .col(
            ColumnDef::new(Alias::new("source_schema_name"))
                .text()
                .not_null(),
        )
        .col(helpers::uuid_col(manager, Alias::new("fork_schema_id")).not_null())
        .col(
            ColumnDef::new(Alias::new("customized"))
                .boolean()
                .not_null()
                .default(false),
        )
        .col(created_at)
        .col(updated_at)
        .foreign_key(
            ForeignKey::create()
                .name("fk_workspace_schema_forks_tenant_id")
                .from(
                    Alias::new("workspace_schema_forks"),
                    Alias::new("tenant_id"),
                )
                .to(Alias::new("tenant_tenants"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .name("fk_workspace_schema_forks_workspace_id")
                .from(
                    Alias::new("workspace_schema_forks"),
                    Alias::new("workspace_id"),
                )
                .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .name("fk_workspace_schema_forks_source_workspace_id")
                .from(
                    Alias::new("workspace_schema_forks"),
                    Alias::new("source_workspace_id"),
                )
                .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Restrict),
        )
        .foreign_key(
            ForeignKey::create()
                .name("fk_workspace_schema_forks_source_schema_id")
                .from(
                    Alias::new("workspace_schema_forks"),
                    Alias::new("source_schema_id"),
                )
                .to(Alias::new("schema_schemas"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Restrict),
        )
        .foreign_key(
            ForeignKey::create()
                .name("fk_workspace_schema_forks_fork_schema_id")
                .from(
                    Alias::new("workspace_schema_forks"),
                    Alias::new("fork_schema_id"),
                )
                .to(Alias::new("schema_schemas"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Restrict),
        )
        .to_owned();

    helpers::create_table_with_checks(
        manager,
        "workspace_schema_forks",
        table,
        &[(
            "workspace_schema_forks_source_schema_version_check",
            "source_schema_version > 0",
        )],
    )
    .await?;

    helpers::pg_only(
                manager,
                "CREATE FUNCTION workspace_schema_fork_source_by_id(p_tenant_id uuid, p_source_workspace_id uuid, p_source_schema_id uuid)
                 RETURNS SETOF schema_schemas
                 LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public AS $$
                   SELECT s.* FROM schema_schemas s
                    WHERE s.tenant_id = p_tenant_id
                      AND s.tenant_id = NULLIF(current_setting('app.current_tenant', true), '')::uuid
                      AND s.workspace_id = p_source_workspace_id
                      AND s.id = p_source_schema_id;
                 $$;
                 CREATE FUNCTION workspace_schema_fork_latest_source(p_tenant_id uuid, p_source_workspace_id uuid, p_source_schema_name text)
                 RETURNS SETOF schema_schemas
                 LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public AS $$
                   SELECT s.* FROM schema_schemas s
                    WHERE s.tenant_id = p_tenant_id
                      AND s.tenant_id = NULLIF(current_setting('app.current_tenant', true), '')::uuid
                      AND s.workspace_id = p_source_workspace_id
                      AND s.name = p_source_schema_name
                      AND s.status = 'active'
                    ORDER BY s.version DESC
                    LIMIT 1;
                 $$;
                 CREATE FUNCTION workspace_schema_fork_edges(p_tenant_id uuid, p_workspace_id uuid)
                 RETURNS TABLE(source_workspace_id uuid)
                 LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public AS $$
                   SELECT f.source_workspace_id
                     FROM workspace_schema_forks f
                    WHERE f.tenant_id = p_tenant_id
                      AND f.tenant_id = NULLIF(current_setting('app.current_tenant', true), '')::uuid
                      AND f.workspace_id = p_workspace_id;
                 $$;
                 REVOKE ALL ON FUNCTION workspace_schema_fork_source_by_id(uuid, uuid, uuid) FROM PUBLIC;
                 REVOKE ALL ON FUNCTION workspace_schema_fork_latest_source(uuid, uuid, text) FROM PUBLIC;
                 REVOKE ALL ON FUNCTION workspace_schema_fork_edges(uuid, uuid) FROM PUBLIC;
                 GRANT EXECUTE ON FUNCTION workspace_schema_fork_source_by_id(uuid, uuid, uuid) TO yorishiro_app;
                 GRANT EXECUTE ON FUNCTION workspace_schema_fork_latest_source(uuid, uuid, text) TO yorishiro_app;
                 GRANT EXECUTE ON FUNCTION workspace_schema_fork_edges(uuid, uuid) TO yorishiro_app;",
            )
            .await?;

    helpers::pg_only(
                manager,
                "CREATE FUNCTION check_workspace_schema_fork_integrity() RETURNS TRIGGER AS $$
                 BEGIN
                   IF NOT EXISTS (
                     SELECT 1 FROM workspace_workspaces
                     WHERE id = NEW.workspace_id AND tenant_id = NEW.tenant_id
                   ) THEN RAISE EXCEPTION 'workspace_schema_forks target workspace mismatch' USING ERRCODE = '23514'; END IF;
                   IF NOT EXISTS (
                     SELECT 1 FROM workspace_workspaces
                     WHERE id = NEW.source_workspace_id AND tenant_id = NEW.tenant_id
                   ) THEN RAISE EXCEPTION 'workspace_schema_forks source workspace mismatch' USING ERRCODE = '23514'; END IF;
                   IF NOT EXISTS (
                     SELECT 1 FROM schema_schemas
                     WHERE id = NEW.source_schema_id AND tenant_id = NEW.tenant_id
                       AND workspace_id = NEW.source_workspace_id
                       AND name = NEW.source_schema_name
                       AND version = NEW.source_schema_version
                   ) THEN RAISE EXCEPTION 'workspace_schema_forks source schema mismatch' USING ERRCODE = '23514'; END IF;
                   IF NOT EXISTS (
                     SELECT 1 FROM schema_schemas
                     WHERE id = NEW.fork_schema_id AND tenant_id = NEW.tenant_id
                       AND workspace_id = NEW.workspace_id
                       AND name = NEW.source_schema_name
                   ) THEN RAISE EXCEPTION 'workspace_schema_forks fork schema mismatch' USING ERRCODE = '23514'; END IF;
                   RETURN NEW;
                 END;
                 $$ LANGUAGE plpgsql SECURITY DEFINER SET search_path = public;
                 CREATE TRIGGER workspace_schema_forks_integrity
                 BEFORE INSERT OR UPDATE ON workspace_schema_forks
                 FOR EACH ROW EXECUTE FUNCTION check_workspace_schema_fork_integrity();",
            )
            .await?;

    helpers::sqlite_only(
        manager,
        "CREATE TRIGGER workspace_schema_forks_integrity_insert
                 BEFORE INSERT ON workspace_schema_forks
                 BEGIN
                   SELECT CASE WHEN NOT EXISTS (
                     SELECT 1 FROM workspace_workspaces
                     WHERE id = NEW.workspace_id AND tenant_id = NEW.tenant_id
                   ) THEN RAISE(ABORT, 'workspace_schema_forks target workspace mismatch') END;
                   SELECT CASE WHEN NOT EXISTS (
                     SELECT 1 FROM workspace_workspaces
                     WHERE id = NEW.source_workspace_id AND tenant_id = NEW.tenant_id
                   ) THEN RAISE(ABORT, 'workspace_schema_forks source workspace mismatch') END;
                   SELECT CASE WHEN NOT EXISTS (
                     SELECT 1 FROM schema_schemas
                     WHERE id = NEW.source_schema_id AND tenant_id = NEW.tenant_id
                       AND workspace_id = NEW.source_workspace_id
                       AND name = NEW.source_schema_name
                       AND version = NEW.source_schema_version
                   ) THEN RAISE(ABORT, 'workspace_schema_forks source schema mismatch') END;
                   SELECT CASE WHEN NOT EXISTS (
                     SELECT 1 FROM schema_schemas
                     WHERE id = NEW.fork_schema_id AND tenant_id = NEW.tenant_id
                       AND workspace_id = NEW.workspace_id
                       AND name = NEW.source_schema_name
                   ) THEN RAISE(ABORT, 'workspace_schema_forks fork schema mismatch') END;
                 END;
                 CREATE TRIGGER workspace_schema_forks_integrity_update
                 BEFORE UPDATE ON workspace_schema_forks
                 BEGIN
                   SELECT CASE WHEN NOT EXISTS (
                     SELECT 1 FROM workspace_workspaces
                     WHERE id = NEW.workspace_id AND tenant_id = NEW.tenant_id
                   ) THEN RAISE(ABORT, 'workspace_schema_forks target workspace mismatch') END;
                   SELECT CASE WHEN NOT EXISTS (
                     SELECT 1 FROM workspace_workspaces
                     WHERE id = NEW.source_workspace_id AND tenant_id = NEW.tenant_id
                   ) THEN RAISE(ABORT, 'workspace_schema_forks source workspace mismatch') END;
                   SELECT CASE WHEN NOT EXISTS (
                     SELECT 1 FROM schema_schemas
                     WHERE id = NEW.source_schema_id AND tenant_id = NEW.tenant_id
                       AND workspace_id = NEW.source_workspace_id
                       AND name = NEW.source_schema_name
                       AND version = NEW.source_schema_version
                   ) THEN RAISE(ABORT, 'workspace_schema_forks source schema mismatch') END;
                   SELECT CASE WHEN NOT EXISTS (
                     SELECT 1 FROM schema_schemas
                     WHERE id = NEW.fork_schema_id AND tenant_id = NEW.tenant_id
                       AND workspace_id = NEW.workspace_id
                       AND name = NEW.source_schema_name
                   ) THEN RAISE(ABORT, 'workspace_schema_forks fork schema mismatch') END;
                 END;",
    )
    .await?;

    manager
        .create_index(
            Index::create()
                .name("workspace_schema_forks_workspace_source_name_key")
                .table(Alias::new("workspace_schema_forks"))
                .col(Alias::new("workspace_id"))
                .col(Alias::new("source_workspace_id"))
                .col(Alias::new("source_schema_name"))
                .unique()
                .to_owned(),
        )
        .await?;
    manager
        .create_index(
            Index::create()
                .name("workspace_schema_forks_workspace_idx")
                .table(Alias::new("workspace_schema_forks"))
                .col(Alias::new("workspace_id"))
                .to_owned(),
        )
        .await?;
    manager
        .create_index(
            Index::create()
                .name("workspace_schema_forks_source_schema_idx")
                .table(Alias::new("workspace_schema_forks"))
                .col(Alias::new("source_schema_id"))
                .col(Alias::new("source_schema_version"))
                .to_owned(),
        )
        .await?;
    manager
        .create_index(
            Index::create()
                .name("workspace_schema_forks_fork_schema_idx")
                .table(Alias::new("workspace_schema_forks"))
                .col(Alias::new("fork_schema_id"))
                .to_owned(),
        )
        .await?;

    helpers::enable_rls_with_policy(
        manager,
        "workspace_schema_forks",
        "workspace_schema_forks_isolation",
        "workspace_id",
        "app.current_workspace",
        true,
    )
    .await?;
    helpers::grant(
        manager,
        "SELECT, INSERT, UPDATE, DELETE",
        "workspace_schema_forks",
    )
    .await?;
    Ok(())
}
