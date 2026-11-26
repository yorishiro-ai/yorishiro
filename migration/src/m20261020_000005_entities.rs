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
        Box::pin(up_entities(manager)).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        Box::pin(down_entities(manager)).await?;
        Ok(())
    }
}

async fn up_entities(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let mut token = ColumnDef::new(Alias::new("embedding_sync_token"));
    token.text().not_null();
    let [created_at, updated_at] = helpers::timestamps(manager);

    manager
        .create_table(
            Table::create()
                .table(Alias::new("entity_entities"))
                .col(helpers::uuidv7_pk(manager))
                .col(token)
                .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
                .col(helpers::uuid_col(manager, Alias::new("schema_id")).not_null())
                .col(
                    ColumnDef::new(Alias::new("schema_version"))
                        .integer()
                        .not_null(),
                )
                .col(ColumnDef::new(Alias::new("entity_type")).text().not_null())
                .col(helpers::json_col(manager, Alias::new("data")).not_null())
                .col(helpers::uuid_col(manager, Alias::new("created_by")))
                .col(helpers::uuid_col(manager, Alias::new("updated_by")))
                .col(created_at)
                .col(updated_at)
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_entity_entities_workspace_id")
                        .from(Alias::new("entity_entities"), Alias::new("workspace_id"))
                        .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_entity_entities_schema_id")
                        .from(Alias::new("entity_entities"), Alias::new("schema_id"))
                        .to(Alias::new("schema_schemas"), Alias::new("id")),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_entity_entities_created_by")
                        .from(Alias::new("entity_entities"), Alias::new("created_by"))
                        .to(Alias::new("user_users"), Alias::new("id"))
                        .on_delete(ForeignKeyAction::SetNull),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_entity_entities_updated_by")
                        .from(Alias::new("entity_entities"), Alias::new("updated_by"))
                        .to(Alias::new("user_users"), Alias::new("id"))
                        .on_delete(ForeignKeyAction::SetNull),
                )
                .to_owned(),
        )
        .await?;

    helpers::sqlite_only(
        manager,
        "CREATE VIRTUAL TABLE entity_fts USING fts5(entity_id UNINDEXED, data, tokenize='trigram');",
    )
    .await?;

    manager
        .create_index(
            Index::create()
                .name("entity_entities_workspace_type_idx")
                .table(Alias::new("entity_entities"))
                .col(Alias::new("workspace_id"))
                .col(Alias::new("entity_type"))
                .col(Alias::new("created_at"))
                .to_owned(),
        )
        .await?;

    create_data_indexes(manager).await?;

    helpers::enable_rls_with_policy(
        manager,
        "entity_entities",
        "workspace_isolation",
        "workspace_id",
        "app.current_workspace",
        false,
    )
    .await?;

    helpers::grant(manager, "SELECT, INSERT, UPDATE, DELETE", "entity_entities").await?;
    Ok(())
}

async fn create_data_indexes(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if manager.get_database_backend() == DbBackend::Postgres {
        manager
            .create_index(
                Index::create()
                    .name("entities_data_gin")
                    .table(Alias::new("entity_entities"))
                    .full_text()
                    .col(
                        IndexColumn::from(Alias::new("data"))
                            .with_operator_class(Alias::new("jsonb_path_ops")),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("entities_data_trgm_idx")
                    .table(Alias::new("entity_entities"))
                    .full_text()
                    .col(
                        IndexColumn::from(Expr::cust("normalize(data::text, nfkc)"))
                            .with_operator_class(Alias::new("gin_trgm_ops")),
                    )
                    .to_owned(),
            )
            .await?;
    }
    Ok(())
}

async fn down_entities(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    helpers::sqlite_only(manager, "DROP TABLE entity_fts;").await?;
    manager
        .drop_table(
            Table::drop()
                .table(Alias::new("entity_entities"))
                .to_owned(),
        )
        .await?;

    Ok(())
}
