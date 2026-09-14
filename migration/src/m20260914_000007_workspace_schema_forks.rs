//! Add workspace-owned fork metadata for versioned schema definitions.

use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
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
             $$ LANGUAGE plpgsql SECURITY DEFINER SET search_path = public;
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

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("workspace_schema_fork_heads"))
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("workspace_schema_forks"))
                    .to_owned(),
            )
            .await?;
        helpers::pg_only(
            manager,
            "DROP FUNCTION IF EXISTS check_workspace_schema_fork_head_integrity();
             DROP FUNCTION IF EXISTS check_workspace_schema_fork_integrity();",
        )
        .await?;
        Ok(())
    }
}
