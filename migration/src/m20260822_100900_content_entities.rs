use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let [created_at, updated_at] = helpers::timestamps();

        manager
            .create_table(
                Table::create()
                    .table(Alias::new("content_entities"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk())
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

        let db = manager.get_connection();

        // sea_query has no pgvector column type, so the embedding column is added with raw SQL.
        // The width is declared here only so the HNSW index below can be built against it: an unconstrained `vector` column cannot carry an HNSW index.
        // The width is dropped again after the index exists, and PostgreSQL keeps the index across that type change, so one index suffices because a workspace's vectors are all one width.
        db.execute_unprepared("ALTER TABLE content_entities ADD COLUMN embedding vector(768)")
            .await?;

        // None of these are expressible through create_index: a multi-column composite, a GIN index with jsonb_path_ops, an HNSW index, and a trigram index over an expression.
        db.execute_unprepared(
            "CREATE INDEX entities_workspace_type_idx ON content_entities (workspace_id, entity_type, created_at)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX entities_data_gin ON content_entities USING GIN (data jsonb_path_ops)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX entities_embedding_hnsw ON content_entities USING hnsw (embedding vector_cosine_ops)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX entities_data_trgm_idx ON content_entities USING gin ((data::text) gin_trgm_ops)",
        )
        .await?;

        // Widen the column back to unconstrained `vector` now that the HNSW index is built.
        db.execute_unprepared("ALTER TABLE content_entities ALTER COLUMN embedding TYPE vector")
            .await?;

        // Strict form, on purpose (old file lines 378-382, 386-387).
        // yorishiro_app sets both app.current_tenant and app.current_workspace on every connection, so reaching this table without a workspace set is a bug.
        // Raising surfaces that bug; a lenient policy would instead read it as an empty workspace and hide it.
        helpers::enable_rls_with_policy(
            db,
            "content_entities",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            false,
        )
        .await?;

        // Old file granted this schema-wide (line 415: GRANT ... ON ALL TABLES IN SCHEMA content).
        // One schema no longer separates content tables from identity tables, so the grant is individualized per table here instead.
        helpers::grant(db, "SELECT, INSERT, UPDATE, DELETE", "content_entities").await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("content_entities"))
                    .to_owned(),
            )
            .await
    }
}
