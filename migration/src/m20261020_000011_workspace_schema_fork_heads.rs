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
        up_workspace_schema_fork_heads(manager).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        down_workspace_schema_fork_heads(manager).await?;
        Ok(())
    }
}

async fn up_workspace_schema_fork_heads(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let history = Table::create()
        .table(Alias::new("workspace_schema_fork_heads"))
        .if_not_exists()
        .col(helpers::uuidv7_pk(manager))
        .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
        .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
        .col(helpers::uuid_col(manager, Alias::new("fork_id")).not_null())
        .col(helpers::uuid_col(manager, Alias::new("schema_id")).not_null())
        .col(helpers::created_at(manager))
        .foreign_key(
            ForeignKey::create()
                .name("fk_workspace_schema_fork_heads_fork_id")
                .from(
                    Alias::new("workspace_schema_fork_heads"),
                    Alias::new("fork_id"),
                )
                .to(Alias::new("workspace_schema_forks"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .name("fk_workspace_schema_fork_heads_schema_id")
                .from(
                    Alias::new("workspace_schema_fork_heads"),
                    Alias::new("schema_id"),
                )
                .to(Alias::new("schema_schemas"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Restrict),
        )
        .to_owned();
    manager.create_table(history).await?;
    helpers::pg_only(
                manager,
                "CREATE FUNCTION check_workspace_schema_fork_head_integrity() RETURNS TRIGGER AS $$
                 BEGIN
                   IF TG_OP = 'UPDATE' THEN
                     RAISE EXCEPTION 'workspace_schema_fork_heads rows are immutable' USING ERRCODE = '23514';
                   END IF;
                   IF NOT EXISTS (
                     SELECT 1 FROM workspace_schema_forks
                     WHERE id = NEW.fork_id AND tenant_id = NEW.tenant_id
                       AND workspace_id = NEW.workspace_id
                   ) THEN RAISE EXCEPTION 'workspace_schema_fork_heads fork mismatch' USING ERRCODE = '23514'; END IF;
                   IF NOT EXISTS (
                     SELECT 1 FROM schema_schemas
                     WHERE id = NEW.schema_id AND tenant_id = NEW.tenant_id
                       AND workspace_id = NEW.workspace_id
                   ) THEN RAISE EXCEPTION 'workspace_schema_fork_heads schema mismatch' USING ERRCODE = '23514'; END IF;
                   RETURN NEW;
                 END;
                 $$ LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, public, pg_temp;
                 CREATE TRIGGER workspace_schema_fork_heads_integrity
                 BEFORE INSERT OR UPDATE ON workspace_schema_fork_heads
                 FOR EACH ROW EXECUTE FUNCTION check_workspace_schema_fork_head_integrity();",
            )
            .await?;
    helpers::sqlite_only(
        manager,
        "CREATE TRIGGER workspace_schema_fork_heads_integrity_insert
                 BEFORE INSERT ON workspace_schema_fork_heads
                 BEGIN
                   SELECT CASE WHEN NOT EXISTS (
                     SELECT 1 FROM workspace_schema_forks
                     WHERE id = NEW.fork_id AND tenant_id = NEW.tenant_id
                       AND workspace_id = NEW.workspace_id
                   ) THEN RAISE(ABORT, 'workspace_schema_fork_heads fork mismatch') END;
                   SELECT CASE WHEN NOT EXISTS (
                     SELECT 1 FROM schema_schemas
                     WHERE id = NEW.schema_id AND tenant_id = NEW.tenant_id
                       AND workspace_id = NEW.workspace_id
                   ) THEN RAISE(ABORT, 'workspace_schema_fork_heads schema mismatch') END;
                 END;
                 CREATE TRIGGER workspace_schema_fork_heads_integrity_update
                 BEFORE UPDATE ON workspace_schema_fork_heads
                 BEGIN
                   SELECT RAISE(ABORT, 'workspace_schema_fork_heads rows are immutable');
                 END;",
    )
    .await?;
    manager
        .create_index(
            Index::create()
                .name("workspace_schema_fork_heads_fork_schema_key")
                .table(Alias::new("workspace_schema_fork_heads"))
                .col(Alias::new("fork_id"))
                .col(Alias::new("schema_id"))
                .unique()
                .to_owned(),
        )
        .await?;
    manager
        .create_index(
            Index::create()
                .name("workspace_schema_fork_heads_workspace_idx")
                .table(Alias::new("workspace_schema_fork_heads"))
                .col(Alias::new("workspace_id"))
                .to_owned(),
        )
        .await?;
    helpers::enable_rls_with_policy(
        manager,
        "workspace_schema_fork_heads",
        "workspace_schema_fork_heads_isolation",
        "workspace_id",
        "app.current_workspace",
        true,
    )
    .await?;
    helpers::grant(
        manager,
        "SELECT, INSERT, UPDATE, DELETE",
        "workspace_schema_fork_heads",
    )
    .await?;

    // Only counts leave the destination workspace's RLS boundary, never fork contents or identifiers.
    helpers::pg_only(manager, "CREATE FUNCTION schema_fork_reference_counts(p_schema_id uuid)
            RETURNS TABLE(forks bigint, fork_heads bigint)
            LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog, public, pg_temp AS $$
              SELECT
                (SELECT count(*) FROM workspace_schema_forks f WHERE f.tenant_id = s.tenant_id AND (f.source_schema_id = s.id OR f.fork_schema_id = s.id)),
                (SELECT count(*) FROM workspace_schema_fork_heads h WHERE h.tenant_id = s.tenant_id AND h.schema_id = s.id)
              FROM schema_schemas s WHERE s.id = p_schema_id
                AND s.workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid
                AND s.tenant_id = NULLIF(current_setting('app.current_tenant', true), '')::uuid;
            $$;
            REVOKE ALL ON FUNCTION schema_fork_reference_counts(uuid) FROM PUBLIC;
            GRANT EXECUTE ON FUNCTION schema_fork_reference_counts(uuid) TO yorishiro_app;").await?;
    Ok(())
}

async fn down_workspace_schema_fork_heads(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    helpers::pg_only(manager, "DROP FUNCTION schema_fork_reference_counts(uuid)").await?;
    helpers::pg_only(manager,"DROP TRIGGER workspace_schema_fork_heads_integrity ON workspace_schema_fork_heads; DROP FUNCTION check_workspace_schema_fork_head_integrity();").await?;
    manager
        .drop_table(
            Table::drop()
                .table(Alias::new("workspace_schema_fork_heads"))
                .to_owned(),
        )
        .await?;

    Ok(())
}
