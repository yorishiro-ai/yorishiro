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
        let mut token = ColumnDef::new(Alias::new("embedding_sync_token"));
        token.text().not_null();
        if manager.get_database_backend() == DbBackend::Postgres {
            token.default(Expr::cust("uuidv7()::text"));
        } else {
            token.default("");
        }
        let [created_at, updated_at] = helpers::timestamps(manager);

        manager
            .create_table(
                Table::create()
                    .table(Alias::new("entity_entities"))
                    .if_not_exists()
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

        if manager.get_database_backend() == sea_orm::DbBackend::Sqlite {
            manager
                .get_connection()
                .execute_unprepared(
                    "CREATE VIRTUAL TABLE entity_fts USING fts5(\
                         entity_id UNINDEXED,\
                         data,\
                         tokenize='trigram'\
                     );",
                )
                .await?;

            manager
                .get_connection()
                .execute_unprepared(
                    "CREATE TRIGGER entity_fts_ai AFTER INSERT ON entity_entities BEGIN \
                         INSERT INTO entity_fts(entity_id, data) \
                         VALUES (CAST(NEW.id AS TEXT), NEW.data); \
                     END;

                     CREATE TRIGGER entity_fts_ad AFTER DELETE ON entity_entities BEGIN \
                         DELETE FROM entity_fts WHERE entity_id = CAST(OLD.id AS TEXT); \
                     END;

                     CREATE TRIGGER entity_fts_au AFTER UPDATE OF data ON entity_entities BEGIN \
                         DELETE FROM entity_fts WHERE entity_id = CAST(OLD.id AS TEXT); \
                         INSERT INTO entity_fts(entity_id, data) \
                         VALUES (CAST(NEW.id AS TEXT), NEW.data); \
                     END;",
                )
                .await?;
        }

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

        helpers::pg_only(
            manager,
            "CREATE INDEX entities_data_gin ON entity_entities USING GIN (data jsonb_path_ops)",
        )
        .await?;
        helpers::pg_only(
                manager,
                "CREATE INDEX entities_data_trgm_idx ON entity_entities USING gin ((data::text) gin_trgm_ops)",
            )
            .await?;

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
        let backend = manager.get_database_backend();
        let conn = manager.get_connection();
        match backend {
            DbBackend::Postgres => {
                conn.execute_unprepared(
                    "DROP TRIGGER IF EXISTS entity_entities_token_on_insert ON entity_entities;",
                )
                .await?;
                conn.execute_unprepared(
                    "DROP TRIGGER IF EXISTS entity_entities_token_on_update ON entity_entities;",
                )
                .await?;

                // Raw strings: a `\` line continuation would join `BEGIN` and `IF` into one token.
                conn.execute_unprepared(
                    r#"CREATE OR REPLACE FUNCTION _set_embedding_sync_token_insert()
RETURNS trigger AS $func$
BEGIN
    IF NEW.embedding_sync_token IS NULL THEN
        NEW.embedding_sync_token := uuidv7()::text;
    END IF;
    RETURN NEW;
END;
$func$ LANGUAGE plpgsql;"#,
                )
                .await?;
                conn.execute_unprepared(
                    "CREATE TRIGGER entity_entities_token_on_insert \
                     BEFORE INSERT ON entity_entities \
                     FOR EACH ROW \
                     EXECUTE FUNCTION _set_embedding_sync_token_insert();",
                )
                .await?;

                conn.execute_unprepared(
                    r#"CREATE OR REPLACE FUNCTION _set_embedding_sync_token_update()
RETURNS trigger AS $func$
BEGIN
    NEW.embedding_sync_token := uuidv7()::text;
    RETURN NEW;
END;
$func$ LANGUAGE plpgsql;"#,
                )
                .await?;
                conn.execute_unprepared(
                    "CREATE TRIGGER entity_entities_token_on_update \
                     BEFORE UPDATE ON entity_entities \
                     FOR EACH ROW \
                     EXECUTE FUNCTION _set_embedding_sync_token_update();",
                )
                .await?;
            }
            DbBackend::Sqlite => {
                conn.execute_unprepared("DROP TRIGGER IF EXISTS entity_entities_token_on_insert;")
                    .await?;
                conn.execute_unprepared("DROP TRIGGER IF EXISTS entity_entities_token_on_update;")
                    .await?;

                conn.execute_unprepared(
                    "CREATE TRIGGER entity_entities_token_on_insert \
                     AFTER INSERT ON entity_entities \
                     FOR EACH ROW \
                     WHEN NEW.embedding_sync_token = '' \
                     BEGIN \
                         UPDATE entity_entities \
                         SET embedding_sync_token = lower(hex(randomblob(16))) \
                         WHERE id = NEW.id; \
                     END;",
                )
                .await?;
                conn.execute_unprepared(
                    "CREATE TRIGGER entity_entities_token_on_update \
                     AFTER UPDATE ON entity_entities \
                     FOR EACH ROW \
                     WHEN NEW.embedding_sync_token = OLD.embedding_sync_token \
                     BEGIN \
                         UPDATE entity_entities \
                         SET embedding_sync_token = lower(hex(randomblob(16))) \
                         WHERE id = NEW.id; \
                     END;",
                )
                .await?;
            }
            _ => {}
        }
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        helpers::pg_only(manager,"DROP TRIGGER entity_entities_token_on_insert ON entity_entities; DROP TRIGGER entity_entities_token_on_update ON entity_entities; DROP FUNCTION _set_embedding_sync_token_insert(); DROP FUNCTION _set_embedding_sync_token_update();").await?;
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
}
