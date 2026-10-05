//! Add the embedding-sync concurrency token to entity_entities.
//!
//! Replaces the `updated_at = $4` timestamp guard in `embed_and_write` with a
//! `embedding_sync_token = $4` string-equality check.
//!
//! Token generation is DB-owned via triggers:
//! - PostgreSQL: `BEFORE INSERT` trigger sets the token from `uuidv7()` (also covers
//!   explicit NULL).  `BEFORE UPDATE` trigger always sets a fresh `uuidv7()`.
//! - SQLite:   the column has no DEFAULT (TEXT columns cannot use function calls in
//!   SQLite's DEFAULT expressions), so the column default is an empty sentinel `''`.
//!   An `AFTER INSERT` trigger rotates the token for every new row whose token is
//!   empty.  An `AFTER UPDATE` trigger rotates the token on every UPDATE whose
//!   statement did not itself change the token, using a plain `UPDATE` with a
//!   `WHEN NEW.embedding_sync_token = OLD.embedding_sync_token` guard so the
//!   nested update does not double-rotate.  Both triggers fire regardless of the
//!   `recursive_triggers` setting: with the default `OFF` they only fire once
//!   because recursive firing is disabled; with `ON` the `WHEN` clause is false on
//!   the nested update (the token was just set, so `NEW != OLD`).
//!
//! - PostgreSQL: `uuidv7()` text — monotonic, globally unique.
//! - SQLite:     `lower(hex(randomblob(16)))` — 32-char hex, unique per write.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DbBackend;

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

        // 1. Add the column with backend-specific defaults.
        //    PostgreSQL: `uuidv7()` — the BEFORE INSERT trigger also covers
        //                explicit NULL, and the BEFORE UPDATE trigger always
        //                rotates.  `Expr::cust("uuidv7()")` (not a string)
        //                avoids quoting the function call as a literal.
        //    SQLite:     sentinel empty string — TEXT columns cannot have
        //                function-call defaults in SQLite, so every row is
        //                seeded below and an INSERT trigger fills gaps.
        let mut col_def = ColumnDef::new(Alias::new("embedding_sync_token"));
        col_def.text().not_null();
        match backend {
            DbBackend::Postgres => {
                col_def.default(Expr::cust("uuidv7()"));
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

        // 2. Seed existing rows so every row has a non-empty unique token.
        match backend {
            DbBackend::Postgres => {
                // uuidv7() default was already applied to every row by the ALTER.
            }
            DbBackend::Sqlite => {
                // All existing rows have the sentinel default; seed them all.
                conn.execute_unprepared(
                    "UPDATE entity_entities \
                     SET embedding_sync_token = lower(hex(randomblob(16)))",
                )
                .await?;
            }
            _ => {}
        }

        // 3. Install triggers that fire on every INSERT and UPDATE,
        //    ensuring the token is always fresh regardless of how the row
        //    is modified (ActiveModel, update_many, raw SQL).
        match backend {
            DbBackend::Postgres => {
                // Drop triggers first for idempotent migration re-runs.
                conn.execute_unprepared(
                    "DROP TRIGGER IF EXISTS entity_entities_token_on_insert ON entity_entities;",
                )
                .await?;
                conn.execute_unprepared(
                    "DROP TRIGGER IF EXISTS entity_entities_token_on_update ON entity_entities;",
                )
                .await?;

                // INSERT trigger: default is uuidv7(), but this ensures
                // even explicit NULL gets replaced.
                conn.execute_unprepared(
                    "CREATE OR REPLACE FUNCTION _set_embedding_sync_token_insert()\
RETURNS trigger AS $func$\
BEGIN\
    IF NEW.embedding_sync_token IS NULL THEN\
        NEW.embedding_sync_token := uuidv7();\
    END IF;\
    RETURN NEW;\
END;\
$func$ LANGUAGE plpgsql;",
                )
                .await?;

                conn.execute_unprepared(
                    "CREATE TRIGGER entity_entities_token_on_insert \
                     BEFORE INSERT ON entity_entities \
                     FOR EACH ROW \
                     EXECUTE FUNCTION _set_embedding_sync_token_insert();",
                )
                .await?;

                // UPDATE trigger: always generate a fresh token on every UPDATE,
                // regardless of whether the application set it or not. This is
                // essential — update_many and raw SQL may not touch the column.
                conn.execute_unprepared(
                    "CREATE OR REPLACE FUNCTION _set_embedding_sync_token_update()\
RETURNS trigger AS $func$\
BEGIN\
    NEW.embedding_sync_token := uuidv7();\
    RETURN NEW;\
END;\
$func$ LANGUAGE plpgsql;",
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
                // Drop any previously created triggers for idempotent re-runs.
                conn.execute_unprepared("DROP TRIGGER IF EXISTS entity_entities_token_on_insert;")
                    .await?;
                conn.execute_unprepared("DROP TRIGGER IF EXISTS entity_entities_token_on_update;")
                    .await?;

                // AFTER INSERT trigger: rotates the token for every new row
                // whose token is the sentinel empty string.
                //
                // With recursive_triggers = ON (non-default), the nested UPDATE
                // would fire the AFTER UPDATE trigger, but the WHEN clause
                // `NEW.token = OLD.token` is false because NEW is the freshly
                // generated random hex while OLD is the empty sentinel — so
                // recursion terminates cleanly.
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

                // AFTER UPDATE trigger: rotates the token on every UPDATE
                // whose statement did not itself change the token.
                //
                // With recursive_triggers = ON (non-default), the nested UPDATE
                // would fire the AFTER UPDATE trigger again, but the WHEN
                // clause `NEW.token = OLD.token` is false because NEW is the
                // freshly generated random hex while OLD is the pre-UPDATE
                // value — so recursion terminates cleanly.
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
        let backend = manager.get_database_backend();

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
