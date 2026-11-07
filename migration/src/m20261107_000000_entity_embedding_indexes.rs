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
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("entity_embedding_indexes"))
                    .col(
                        helpers::uuid_col(manager, Alias::new("workspace_id"))
                            .not_null()
                            .primary_key(),
                    )
                    .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
                    .col(
                        ColumnDef::new(Alias::new("revision"))
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(Alias::new("dimensions")).integer())
                    .col(ColumnDef::new(Alias::new("format")).text())
                    .col(ColumnDef::new(Alias::new("graph")).binary())
                    .col(ColumnDef::new(Alias::new("vectors")).binary())
                    .col(
                        helpers::json_col(manager, Alias::new("pending"))
                            .not_null()
                            .default("{}"),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                Alias::new("entity_embedding_indexes"),
                                Alias::new("workspace_id"),
                            )
                            .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        helpers::enable_rls_with_policy(
            manager,
            "entity_embedding_indexes",
            "tenant_isolation",
            "tenant_id",
            "app.current_tenant",
            false,
        )
        .await?;
        helpers::grant(manager, "SELECT", "entity_embedding_indexes").await?;
        // Database triggers cover raw SQL writers and roll back with the authoritative vector write.
        helpers::pg_only(manager, r#"
            CREATE FUNCTION initialize_embedding_index() RETURNS trigger
            LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
            BEGIN
                INSERT INTO entity_embedding_indexes(workspace_id, tenant_id) VALUES (NEW.id, NEW.tenant_id);
                RETURN NEW;
            END $$;
            CREATE TRIGGER workspace_embedding_index AFTER INSERT ON workspace_workspaces
                FOR EACH ROW EXECUTE FUNCTION initialize_embedding_index();
            INSERT INTO entity_embedding_indexes(workspace_id, tenant_id)
                SELECT id, tenant_id FROM workspace_workspaces;
            CREATE FUNCTION invalidate_embedding_index() RETURNS trigger
            LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
            BEGIN
                IF TG_OP = 'DELETE' OR TG_OP = 'UPDATE' THEN
                    PERFORM pg_advisory_xact_lock(hashtextextended('embedding-hnsw:' || OLD.workspace_id::text, 0));
                    UPDATE entity_embedding_indexes SET revision = revision + 1,
                         pending = CASE WHEN graph IS NULL THEN pending ELSE pending || jsonb_build_object(OLD.entity_id::text, true) END WHERE workspace_id = OLD.workspace_id;
                END IF;
                IF TG_OP = 'INSERT' OR (TG_OP = 'UPDATE' AND (OLD.workspace_id IS DISTINCT FROM NEW.workspace_id OR OLD.entity_id IS DISTINCT FROM NEW.entity_id)) THEN
                    PERFORM pg_advisory_xact_lock(hashtextextended('embedding-hnsw:' || NEW.workspace_id::text, 0));
                    UPDATE entity_embedding_indexes SET revision = revision + 1,
                         pending = CASE WHEN graph IS NULL THEN pending ELSE pending || jsonb_build_object(NEW.entity_id::text, true) END WHERE workspace_id = NEW.workspace_id;
                END IF;
                IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
                RETURN NEW;
            END $$;
            CREATE TRIGGER embedding_index_invalidation BEFORE INSERT OR UPDATE OR DELETE ON entity_embeddings
                FOR EACH ROW EXECUTE FUNCTION invalidate_embedding_index();
        "#).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        helpers::pg_only(manager, "DROP TRIGGER embedding_index_invalidation ON entity_embeddings; DROP FUNCTION invalidate_embedding_index(); DROP TRIGGER workspace_embedding_index ON workspace_workspaces; DROP FUNCTION initialize_embedding_index();").await?;
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("entity_embedding_indexes"))
                    .to_owned(),
            )
            .await
    }
}
