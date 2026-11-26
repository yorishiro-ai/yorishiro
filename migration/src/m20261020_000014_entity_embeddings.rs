use super::helpers;
use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DbBackend;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        Box::pin(up_entity_embeddings(manager)).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        Box::pin(down_entity_embeddings(manager)).await?;
        Ok(())
    }
}

async fn up_entity_embeddings(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    {
        let name = "entity_embeddings";
        let embedding = helpers::by_backend(
            manager,
            ColumnDef::new(Alias::new("embedding"))
                .vector(None)
                .to_owned(),
            ColumnDef::new(Alias::new("embedding")).blob().to_owned(),
        );
        let mut table = Table::create()
            .table(Alias::new(name))
            .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
            .col(helpers::uuid_col(manager, Alias::new("entity_id")).not_null())
            .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
            .col(embedding)
            .primary_key(
                Index::create()
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("entity_id")),
            )
            .foreign_key(
                ForeignKey::create()
                    .from(Alias::new(name), Alias::new("entity_id"))
                    .to(Alias::new("entity_entities"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .to_owned();
        if manager.get_database_backend() == DbBackend::Postgres {
            table.partition_by_hash([Alias::new("workspace_id")]);
        }
        manager.create_table(table).await?;
        if manager.get_database_backend() == DbBackend::Postgres {
            for remainder in 0..64 {
                manager
                    .create_table(
                        Table::create()
                            .table(Alias::new(format!("{name}_p{remainder}")))
                            .partition_of(Alias::new(name))
                            .values_with(64, remainder)
                            .to_owned(),
                    )
                    .await?;
            }
        }
        manager
            .create_index(
                Index::create()
                    .name("entity_embeddings_workspace_idx")
                    .table(Alias::new(name))
                    .col(Alias::new("workspace_id"))
                    .to_owned(),
            )
            .await?;
        helpers::enable_rls_with_policy(
            manager,
            name,
            "tenant_isolation",
            "tenant_id",
            "app.current_tenant",
            false,
        )
        .await?;
        helpers::grant(manager, "SELECT", name).await?;
    }

    Ok(())
}

async fn down_entity_embeddings(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    {
        let name = "entity_embeddings";
        manager
            .drop_table(Table::drop().table(Alias::new(name)).to_owned())
            .await?;
    }
    Ok(())
}
