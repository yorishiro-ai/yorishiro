use super::helpers;
use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DbBackend;
#[derive(DeriveMigrationName)]
pub struct Migration;
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("schema_schemas"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
                    .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
                    .col(ColumnDef::new(Alias::new("name")).text().not_null())
                    .col(
                        ColumnDef::new(Alias::new("version"))
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .col(helpers::json_col(manager, Alias::new("definition")).not_null())
                    .col(
                        ColumnDef::new(Alias::new("status"))
                            .text()
                            .not_null()
                            .default("active"),
                    )
                    .col(helpers::uuid_col(manager, Alias::new("origin_template_id")))
                    .col(
                        ColumnDef::new(Alias::new("origin_status"))
                            .text()
                            .not_null()
                            .default("detached"),
                    )
                    .col(helpers::json_col(manager, Alias::new("origin_snapshot")))
                    .col(helpers::ts_tz_col(manager, Alias::new("origin_updated_at")))
                    .col(helpers::created_at(manager))
                    .col(
                        helpers::ts_tz_col(manager, Alias::new("updated_at"))
                            .null()
                            .default(Expr::current_timestamp()),
                    )
                    .check(Expr::cust("status IN ('active', 'archived')"))
                    .check(Expr::cust("origin_status IN ('linked', 'detached')"))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_schema_schemas_tenant_id")
                            .from(Alias::new("schema_schemas"), Alias::new("tenant_id"))
                            .to(Alias::new("tenant_tenants"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_schema_schemas_workspace_id")
                            .from(Alias::new("schema_schemas"), Alias::new("workspace_id"))
                            .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_schema_schemas_origin_template_id")
                            .from(
                                Alias::new("schema_schemas"),
                                Alias::new("origin_template_id"),
                            )
                            .to(Alias::new("template_templates"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        if manager.get_database_backend() == DbBackend::Postgres {
            manager
                .create_foreign_key(
                    ForeignKey::create()
                        .name("fk_workspace_workspaces_schema_id")
                        .from(Alias::new("workspace_workspaces"), Alias::new("schema_id"))
                        .to(Alias::new("schema_schemas"), Alias::new("id"))
                        .to_owned(),
                )
                .await?;
        }

        manager
            .create_index(
                Index::create()
                    .name("schema_schemas_workspace_name_version_key")
                    .table(Alias::new("schema_schemas"))
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
                    .name("schema_schemas_tenant_id_idx")
                    .table(Alias::new("schema_schemas"))
                    .col(Alias::new("tenant_id"))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("schema_schemas_workspace_id_idx")
                    .table(Alias::new("schema_schemas"))
                    .col(Alias::new("workspace_id"))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("schema_schemas_origin_template_idx")
                    .table(Alias::new("schema_schemas"))
                    .col(Alias::new("origin_template_id"))
                    .and_where(Expr::col(Alias::new("origin_template_id")).is_not_null())
                    .to_owned(),
            )
            .await?;

        helpers::pg_only(
            manager,
            &format!(
                "CREATE FUNCTION detach_orphaned_schema_origin() RETURNS TRIGGER AS $$
                     BEGIN
                       {}
                       RETURN OLD;
                     END;
                     $$ LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, public, pg_temp;",
                detach_body("now()")
            ),
        )
        .await?;

        helpers::pg_only(
            manager,
            "CREATE TRIGGER template_templates_detach_schema_origins
                 BEFORE DELETE ON template_templates
                 FOR EACH ROW EXECUTE FUNCTION detach_orphaned_schema_origin();",
        )
        .await?;

        helpers::sqlite_only(
            manager,
            &format!(
                "CREATE TRIGGER template_templates_detach_schema_origins
                     BEFORE DELETE ON template_templates
                     FOR EACH ROW
                     BEGIN
                       {}
                     END;",
                detach_body("CURRENT_TIMESTAMP")
            ),
        )
        .await?;

        helpers::enable_rls_with_policy(
            manager,
            "schema_schemas",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            true,
        )
        .await?;

        helpers::grant(manager, "SELECT, INSERT, UPDATE, DELETE", "schema_schemas").await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite cannot drop the circular foreign key independently of its table.
        if manager.get_database_backend() == DbBackend::Sqlite {
            manager
                .get_connection()
                .execute(
                    &Query::update()
                        .table(Alias::new("workspace_workspaces"))
                        .value(Alias::new("schema_id"), Option::<String>::None)
                        .to_owned(),
                )
                .await?;
        }
        if manager.get_database_backend() == DbBackend::Postgres {
            manager
                .drop_foreign_key(
                    ForeignKey::drop()
                        .name("fk_workspace_workspaces_schema_id")
                        .table(Alias::new("workspace_workspaces"))
                        .to_owned(),
                )
                .await?;
        }
        helpers::pg_only(manager, "DROP TRIGGER template_templates_detach_schema_origins ON template_templates; DROP FUNCTION detach_orphaned_schema_origin();").await?;
        helpers::sqlite_only(
            manager,
            "DROP TRIGGER template_templates_detach_schema_origins;",
        )
        .await?;

        manager
            .drop_table(Table::drop().table(Alias::new("schema_schemas")).to_owned())
            .await?;

        Ok(())
    }
}
fn detach_body(now: &str) -> String {
    format!(
        "UPDATE schema_schemas \
            SET origin_status = 'detached', updated_at = {now} \
          WHERE origin_template_id = OLD.id \
            AND origin_status = 'linked';"
    )
}
