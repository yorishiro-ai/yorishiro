//! Per-width embedding tables for #299.
//!
//! Each table holds vectors from one embedding model width (768, 1024, 1536).
//! This lets a single deployment host workspaces with different models
//! (and different output dimensions) without widening a single column to
//! the largest width and wasting space on every row.
//!
//! Tables follow the same backend split as the original
//! `content_entity_embeddings`:
//! - PostgreSQL: `vector(N)` with HNSW index.
//! - SQLite: `BLOB` (no index, KNN via `vec_distance_cosine`).
//!
//! A CASCADE delete on `entity_id` would need to hit every table.
//! Instead, `content_entities` has no FK on `entity_id`, so deletes
//! cascade from `content_entities(id)` → `content_entity_embeddings(id)`.
//! The per-width tables use `ON DELETE CASCADE` on `entity_id` so a
//! deleted entity drops its vector from whatever table holds it.

use super::helpers;
use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DbBackend;

/// Common widths this migration creates tables for.
/// A table for a width that no workspace uses is harmless: it is simply
/// never queried, and only occupies negligible catalog metadata.
/// When a deployment needs a new width, the helper below creates the table.
const WIDTHS: &[i32] = &[768, 1024, 1536];

/// The table name for a given width.
fn table_name(width: i32) -> String {
    format!("content_entity_embeddings_{width}")
}

/// The `vector(N)` type for PostgreSQL.
fn pg_vector_type(width: i32) -> String {
    format!("vector({width})")
}

/// The `HNSW` index name for a table.
fn hnsw_index_name(width: i32) -> String {
    let tname = table_name(width);
    format!("idx_{tname}_hnsw")
}

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();

        for &width in WIDTHS {
            let tname = table_name(width);
            let idxname = hnsw_index_name(width);
            let vtype = pg_vector_type(width);

            if backend == DbBackend::Sqlite {
                // SQLite: BLOB, FK cascade only (no HNSW, no vector type).
                manager
                    .get_connection()
                    .execute_unprepared(&format!(
                        "CREATE TABLE {tname} (\
                         entity_id BLOB PRIMARY KEY, \
                         embedding BLOB, \
                         FOREIGN KEY (entity_id) REFERENCES content_entities(id) ON DELETE CASCADE)"
                    ))
                    .await?;
            } else {
                // PostgreSQL: vector(N) with HNSW index.
                manager
                    .get_connection()
                    .execute_unprepared(&format!(
                        "CREATE TABLE {tname} (\
                         entity_id UUID PRIMARY KEY, \
                         embedding {vtype})"
                    ))
                    .await?;

                manager
                    .get_connection()
                    .execute_unprepared(&format!(
                        "CREATE INDEX {idxname} ON {tname} \
                         USING hnsw (embedding vector_cosine_ops)"
                    ))
                    .await?;
            }
        }

        // Migrate existing data from the original `content_entity_embeddings`
        // into the width-specific tables.
        //
        // The deployment default width (YORISHIRO_EMBEDDING_DIMENSIONS, default 768)
        // is used as the target for existing vectors. This is correct because
        // `content_entity_embeddings.embedding` is `vector(768)` at the SQL type
        // level, meaning every existing row is already 768-dimensional.
        if WIDTHS.contains(&768) {
            let target = table_name(768);

            if backend == DbBackend::Sqlite {
                // SQLite: raw LE f32 bytes already match the target format.
                manager
                    .get_connection()
                    .execute_unprepared(&format!(
                        "INSERT INTO {target} (entity_id, embedding) \
                         SELECT entity_id, embedding \
                         FROM content_entity_embeddings \
                         WHERE embedding IS NOT NULL \
                         ON CONFLICT(entity_id) DO NOTHING"
                    ))
                    .await?;
            } else {
                // PostgreSQL: PgVector format.
                manager
                    .get_connection()
                    .execute_unprepared(&format!(
                        "INSERT INTO {target} (entity_id, embedding) \
                         SELECT entity_id, embedding::vector(768) \
                         FROM content_entity_embeddings \
                         WHERE embedding IS NOT NULL \
                         ON CONFLICT(entity_id) DO NOTHING"
                    ))
                    .await?;
            }

            // Drop the original table after migration.
            if backend == DbBackend::Sqlite {
                // SQLite needs CASCADE because other tables may still have FKs.
                // The original content_entity_embeddings has no other dependents
                // at this point (the per-width tables have no FK to it, they
                // only reference content_entities directly).
                manager
                    .drop_table(
                        Table::drop()
                            .table(Alias::new("content_entity_embeddings"))
                            .if_exists()
                            .to_owned(),
                    )
                    .await?;
            } else {
                // PostgreSQL: CASCADE to be safe if any index or constraint
                // still references it (should not at this point).
                manager
                    .drop_table(
                        Table::drop()
                            .table(Alias::new("content_entity_embeddings"))
                            .if_exists()
                            .cascade()
                            .to_owned(),
                    )
                    .await?;
            }
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();

        if backend == DbBackend::Sqlite {
            // SQLite: recreate the original table, migrate data back.
            manager
                .create_table(
                    Table::create()
                        .table(Alias::new("content_entity_embeddings"))
                        .if_not_exists()
                        .col(
                            ColumnDef::new(Alias::new("entity_id"))
                                .blob()
                                .not_null()
                                .primary_key(),
                        )
                        .col(ColumnDef::new(Alias::new("embedding")).blob())
                        .foreign_key(
                            ForeignKey::create()
                                .name("fk_entity_embeddings_entity_id")
                                .from(
                                    Alias::new("content_entity_embeddings"),
                                    Alias::new("entity_id"),
                                )
                                .to(Alias::new("content_entities"), Alias::new("id"))
                                .on_delete(ForeignKeyAction::Cascade),
                        )
                        .to_owned(),
                )
                .await?;

            if WIDTHS.contains(&768) {
                manager
                    .get_connection()
                    .execute_unprepared(&format!(
                        "INSERT INTO content_entity_embeddings (entity_id, embedding) \
                         SELECT entity_id, embedding \
                         FROM {}",
                        table_name(768)
                    ))
                    .await?;
            }
        } else {
            // PostgreSQL: recreate the original table.
            manager
                .create_table(
                    Table::create()
                        .table(Alias::new("content_entity_embeddings"))
                        .if_not_exists()
                        .col(
                            ColumnDef::new(Alias::new("entity_id"))
                                .uuid()
                                .not_null()
                                .primary_key(),
                        )
                        .col(ColumnDef::new(Alias::new("embedding")).custom("vector(768)"))
                        .foreign_key(
                            ForeignKey::create()
                                .name("fk_entity_embeddings_entity_id")
                                .from(
                                    Alias::new("content_entity_embeddings"),
                                    Alias::new("entity_id"),
                                )
                                .to(Alias::new("content_entities"), Alias::new("id"))
                                .on_delete(ForeignKeyAction::Cascade),
                        )
                        .to_owned(),
                )
                .await?;

            manager
                .get_connection()
                .execute_unprepared(
                    "CREATE INDEX entities_embedding_hnsw ON content_entity_embeddings \
                     USING hnsw (embedding vector_cosine_ops)",
                )
                .await?;

            if WIDTHS.contains(&768) {
                manager
                    .get_connection()
                    .execute_unprepared(&format!(
                        "INSERT INTO content_entity_embeddings (entity_id, embedding) \
                         SELECT entity_id, embedding \
                         FROM {}",
                        table_name(768)
                    ))
                    .await?;
            }
        }

        // Drop per-width tables.
        for &width in WIDTHS {
            let tname = table_name(width);
            manager
                .drop_table(
                    Table::drop()
                        .table(Alias::new(&tname))
                        .if_exists()
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }
}
