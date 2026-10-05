//! Add the embedding-sync concurrency token to `entity_entities`.
//!
//! The token replaces the `updated_at = $4` timestamp guard in `embed_and_write` with an `embedding_sync_token = $4` string equality check.
//! Two backends encode `updated_at` differently, so a timestamp can never be compared portably.
//!
//! The database owns token generation, so no write path can forget to rotate it.
//!
//! PostgreSQL:
//! - The column defaults to `uuidv7()::text`.
//! - A `BEFORE UPDATE` trigger assigns a fresh `uuidv7()` on every update, including `update_many` and raw SQL.
//! - A `BEFORE INSERT` trigger replaces an explicit NULL.
//!
//! SQLite:
//! - `ALTER TABLE ADD COLUMN` rejects a non-constant default, so the column defaults to the empty sentinel `''`.
//! - An `AFTER INSERT` trigger replaces the sentinel with `lower(hex(randomblob(16)))`.
//! - An `AFTER UPDATE` trigger rotates the token for every update that did not change it, with a plain `UPDATE` guarded by `WHEN NEW.embedding_sync_token = OLD.embedding_sync_token`.
//! - The guard is false on the nested update, so the trigger ends after one rotation whatever `recursive_triggers` is set to.
//! - The token triggers issue a nested `UPDATE` that sets only the token.
//! - The initial schema's `entity_fts_au` fires on every update, so that nested update would index each inserted entity twice.
//! - This migration therefore narrows `entity_fts_au` to `AFTER UPDATE OF data`, and `down` restores the original `AFTER UPDATE`.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DbBackend;

/// The `entity_fts_au` trigger from the initial schema, with `event` as its firing clause.
fn fts_update_trigger(event: &str) -> String {
    format!(
        "CREATE TRIGGER entity_fts_au {event} ON entity_entities BEGIN \
         DELETE FROM entity_fts WHERE entity_id = CAST(OLD.id AS TEXT); \
         INSERT INTO entity_fts(entity_id, data) \
         VALUES (CAST(NEW.id AS TEXT), NEW.data); \
         END;"
    )
}

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        crate::helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let conn = manager.get_connection();

        // `Expr::cust` keeps the function call from being quoted as a string literal.
        let mut col_def = ColumnDef::new(Alias::new("embedding_sync_token"));
        col_def.text().not_null();
        match backend {
            DbBackend::Postgres => {
                col_def.default(Expr::cust("uuidv7()::text"));
            }
            _ => {
                col_def.default(Expr::cust("''"));
            }
        }
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("entity_entities"))
                    .add_column(col_def)
                    .to_owned(),
            )
            .await?;

        // PostgreSQL applied the default to every existing row during the ALTER.
        // SQLite rows all hold the sentinel and need a real token.
        if backend == DbBackend::Sqlite {
            conn.execute_unprepared(
                "UPDATE entity_entities SET embedding_sync_token = lower(hex(randomblob(16)))",
            )
            .await?;
        }

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
                conn.execute_unprepared(&format!(
                    "DROP TRIGGER IF EXISTS entity_fts_au; {}",
                    fts_update_trigger("AFTER UPDATE OF data")
                ))
                .await?;

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
        let conn = manager.get_connection();

        match manager.get_database_backend() {
            DbBackend::Postgres => {
                conn.execute_unprepared(
                    "DROP TRIGGER IF EXISTS entity_entities_token_on_insert ON entity_entities;",
                )
                .await?;
                conn.execute_unprepared(
                    "DROP TRIGGER IF EXISTS entity_entities_token_on_update ON entity_entities;",
                )
                .await?;
                conn.execute_unprepared(
                    "DROP FUNCTION IF EXISTS _set_embedding_sync_token_insert();",
                )
                .await?;
                conn.execute_unprepared(
                    "DROP FUNCTION IF EXISTS _set_embedding_sync_token_update();",
                )
                .await?;
            }
            DbBackend::Sqlite => {
                conn.execute_unprepared("DROP TRIGGER IF EXISTS entity_entities_token_on_insert;")
                    .await?;
                conn.execute_unprepared("DROP TRIGGER IF EXISTS entity_entities_token_on_update;")
                    .await?;
                conn.execute_unprepared(&format!(
                    "DROP TRIGGER IF EXISTS entity_fts_au; {}",
                    fts_update_trigger("AFTER UPDATE")
                ))
                .await?;
            }
            _ => {}
        }

        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("entity_entities"))
                    .drop_column(Alias::new("embedding_sync_token"))
                    .to_owned(),
            )
            .await
    }
}
