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
                    .table(Alias::new("identity_workspaces"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk())
                    .col(ColumnDef::new(Alias::new("tenant_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("name")).text().not_null())
                    .col(ColumnDef::new(Alias::new("max_entities")).integer())
                    // A workspace exists before its schema does: `admin create-workspace` leaves
                    // it pending, and creating the schema marks it active.
                    .col(
                        ColumnDef::new(Alias::new("status"))
                            .text()
                            .not_null()
                            .default("schema_pending"),
                    )
                    // The model a workspace's vectors were produced by, and their width. NULL
                    // means the deployment default, recorded so a workspace whose model changed
                    // can be told from one provisioned under a different one.
                    .col(ColumnDef::new(Alias::new("embedding_model")).text())
                    .col(ColumnDef::new(Alias::new("embedding_dimensions")).integer())
                    // `schema_id` is added after `content.schemas` exists in the old DDL: the
                    // reference is circular, since a schema also names its workspace. Folded into
                    // this same create_table call as a plain nullable column with NO foreign_key
                    // constraint, since content_schemas is ported by a different, concurrent
                    // migration and may not exist yet when this one runs. The FK constraint
                    // should be added in a later migration once content_schemas is guaranteed to
                    // exist.
                    .col(ColumnDef::new(Alias::new("schema_id")).uuid())
                    .col(helpers::created_at())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_workspaces_tenant_id")
                            .from(Alias::new("identity_workspaces"), Alias::new("tenant_id"))
                            .to(Alias::new("identity_tenants"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        let db = manager.get_connection();

        // CHECK constraints: sea_query's Table::create().check() did not compile cleanly here,
        // so both are added via raw ALTER TABLE right after create_table, matching the old DDL's
        // two check clauses verbatim (lines 97-98 and 103).
        db.execute_unprepared(
            "ALTER TABLE identity_workspaces \
             ADD CONSTRAINT identity_workspaces_status_check \
             CHECK (status IN ('schema_pending', 'active'));",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE identity_workspaces \
             ADD CONSTRAINT identity_workspaces_embedding_dimensions_check \
             CHECK (embedding_dimensions IS NULL OR embedding_dimensions > 0);",
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

        // Strict form: `yorishiro_app` sets both GUCs on every connection, so reaching this
        // table without a tenant is a bug, and raising surfaces it rather than reading as an
        // empty tenant (old DDL lines 378-382, 362-363).
        helpers::enable_rls_with_policy(
            db,
            "identity_workspaces",
            "tenant_isolation",
            "tenant_id",
            "app.current_tenant",
            false,
        )
        .await?;

        // Whole-table SELECT (old DDL line 405).
        helpers::grant(db, "SELECT", "identity_workspaces").await?;

        // Column-level GRANT UPDATE, because these two are the whole of what a request writes
        // here. `max_entities`, `name` and the embedding stamp are provisioning decisions, and a
        // request that could rewrite its own quota is a different system (old DDL lines 407-410).
        // Issued raw since helpers::grant()'s signature only expresses whole-table grants.
        db.execute_unprepared(
            "GRANT UPDATE (status, schema_id) ON identity_workspaces TO yorishiro_app;",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("identity_workspaces"))
                    .to_owned(),
            )
            .await
    }
}
