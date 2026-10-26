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
        {
            let name = "entity_embeddings";
            let mut embedding = ColumnDef::new(Alias::new("embedding"));
            if manager.get_database_backend() == DbBackend::Postgres {
                embedding.custom(Alias::new("vector"));
            } else {
                embedding.blob();
            }
            manager
                .create_table(
                    Table::create()
                        .table(Alias::new(name))
                        .col(
                            helpers::uuid_col(manager, Alias::new("entity_id"))
                                .not_null()
                                .primary_key(),
                        )
                        .col(embedding)
                        .foreign_key(
                            ForeignKey::create()
                                .from(Alias::new(name), Alias::new("entity_id"))
                                .to(Alias::new("entity_entities"), Alias::new("id"))
                                .on_delete(ForeignKeyAction::Cascade),
                        )
                        .to_owned(),
                )
                .await?;
            helpers::pg_only(manager,&format!("ALTER TABLE {name} ENABLE ROW LEVEL SECURITY; CREATE POLICY workspace_isolation ON {name} FOR SELECT USING (EXISTS (SELECT 1 FROM entity_entities e WHERE e.id = {name}.entity_id));")).await?;
            helpers::grant(manager, "SELECT", name).await?;
        }

        // Fence entity and provider mutations against embedding publication, including raw SQL writers.
        helpers::pg_only(manager, r#"
            CREATE FUNCTION touch_workspace_embedding_revision() RETURNS trigger
            LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
            BEGIN
                IF TG_TABLE_NAME = 'entity_entities' THEN
                    IF TG_OP = 'UPDATE' AND OLD.workspace_id IS DISTINCT FROM NEW.workspace_id THEN
                        UPDATE workspace_workspaces SET embedding_content_revision = embedding_content_revision + 1 WHERE id = OLD.workspace_id;
                    END IF;
                    IF TG_OP = 'DELETE' THEN
                        UPDATE workspace_workspaces SET embedding_content_revision = embedding_content_revision + 1 WHERE id = OLD.workspace_id;
                    ELSE
                        UPDATE workspace_workspaces SET embedding_content_revision = embedding_content_revision + 1 WHERE id = NEW.workspace_id;
                    END IF;
                ELSE
                    IF TG_OP = 'DELETE' THEN
                        UPDATE workspace_workspaces SET embedding_revision = embedding_revision + 1, embedding_reindexing = true WHERE id = OLD.workspace_id;
                    ELSE
                        UPDATE workspace_workspaces SET embedding_revision = embedding_revision + 1, embedding_reindexing = true WHERE id = NEW.workspace_id;
                    END IF;
                END IF;
                IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
                RETURN NEW;
            END $$;
            CREATE TRIGGER entity_embedding_revision BEFORE INSERT OR DELETE OR UPDATE OF data, schema_id, entity_type, workspace_id
                ON entity_entities FOR EACH ROW EXECUTE FUNCTION touch_workspace_embedding_revision();
            CREATE TRIGGER provider_embedding_revision BEFORE INSERT OR UPDATE OR DELETE
                ON workspace_embedding_keys FOR EACH ROW EXECUTE FUNCTION touch_workspace_embedding_revision();
        "#).await?;
        for table in ["entity_entities", "workspace_embedding_keys"] {
            for (operation, row) in [("INSERT", "NEW"), ("UPDATE", "NEW"), ("DELETE", "OLD")] {
                let operation_sql = if table == "entity_entities" && operation == "UPDATE" {
                    "UPDATE OF data, schema_id, entity_type, workspace_id"
                } else {
                    operation
                };
                helpers::sqlite_only(manager, &format!(
                    "CREATE TRIGGER {table}_embedding_revision_{operation} BEFORE {operation_sql} ON {table} BEGIN \
                     UPDATE workspace_workspaces SET {revision} = {revision} + 1{gate} WHERE id = {row}.workspace_id; END;",
                     revision = if table == "entity_entities" { "embedding_content_revision" } else { "embedding_revision" },
                     gate = if table == "entity_entities" { "" } else { ", embedding_reindexing = true" },
                )).await?;
            }
        }
        helpers::sqlite_only(manager, "CREATE TRIGGER entity_embedding_revision_move BEFORE UPDATE OF workspace_id ON entity_entities WHEN OLD.workspace_id != NEW.workspace_id BEGIN UPDATE workspace_workspaces SET embedding_content_revision = embedding_content_revision + 1 WHERE id = OLD.workspace_id; END;").await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        helpers::sqlite_only(manager, "DROP TRIGGER entity_embedding_revision_move;").await?;
        helpers::pg_only(manager, "DROP TRIGGER entity_embedding_revision ON entity_entities; DROP TRIGGER provider_embedding_revision ON workspace_embedding_keys; DROP FUNCTION touch_workspace_embedding_revision();").await?;
        for table in ["entity_entities", "workspace_embedding_keys"] {
            for operation in ["INSERT", "UPDATE", "DELETE"] {
                helpers::sqlite_only(
                    manager,
                    &format!("DROP TRIGGER {table}_embedding_revision_{operation};"),
                )
                .await?;
            }
        }
        {
            let name = "entity_embeddings";
            manager
                .drop_table(Table::drop().table(Alias::new(name)).to_owned())
                .await?;
        }
        Ok(())
    }
}
