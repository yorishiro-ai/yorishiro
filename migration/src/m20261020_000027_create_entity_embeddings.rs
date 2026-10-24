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
        for width in [768, 1024, 1536] {
            let name = format!("entity_embeddings_{width}");
            let mut embedding = ColumnDef::new(Alias::new("embedding"));
            if manager.get_database_backend() == DbBackend::Postgres {
                embedding.custom(Alias::new(format!("vector({width})")));
            } else {
                embedding.blob();
            }
            manager
                .create_table(
                    Table::create()
                        .table(Alias::new(&name))
                        .col(
                            helpers::uuid_col(manager, Alias::new("entity_id"))
                                .not_null()
                                .primary_key(),
                        )
                        .col(embedding)
                        .foreign_key(
                            ForeignKey::create()
                                .from(Alias::new(&name), Alias::new("entity_id"))
                                .to(Alias::new("entity_entities"), Alias::new("id"))
                                .on_delete(ForeignKeyAction::Cascade),
                        )
                        .to_owned(),
                )
                .await?;
            // HNSW operator classes and RLS are PostgreSQL-specific.
            helpers::pg_only(manager,&format!("CREATE INDEX idx_{name}_hnsw ON {name} USING hnsw (embedding vector_cosine_ops); ALTER TABLE {name} ENABLE ROW LEVEL SECURITY; CREATE POLICY workspace_isolation ON {name} FOR SELECT USING (EXISTS (SELECT 1 FROM entity_entities e WHERE e.id = {name}.entity_id));")).await?;
            helpers::grant(manager, "SELECT", &name).await?;
        }

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for width in [768, 1024, 1536] {
            manager
                .drop_table(
                    Table::drop()
                        .table(Alias::new(format!("entity_embeddings_{width}")))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}
