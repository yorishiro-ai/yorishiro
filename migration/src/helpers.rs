//! Column and statement builders shared by the migrations.
//!
//! SQLite has no roles, RLS or SECURITY DEFINER functions.
//! `enable_rls_with_policy`, `grant` and `pg_only` no-op there, so a migration's PostgreSQL statements stay unbranched.
use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DbBackend;

/// Forces every migration to run inside one transaction, on every backend.
/// SQLite does not run migrations inside a transaction by default, so a failed statement can leave a partially applied schema.
#[expect(
    clippy::unnecessary_wraps,
    reason = "matches MigrationTrait::use_transaction so every migration delegates the same transaction policy"
)]
pub(super) const fn use_transaction() -> Option<bool> {
    Some(true)
}

/// `id UUID PRIMARY KEY` on Postgres, `id BLOB PRIMARY KEY` on SQLite.
///
/// The application supplies every id (see `crate::db::generated_id`), so there is no database default.
/// `SeaORM` binds `Uuid` as a 16-byte BLOB, so the SQLite column is BLOB too.
pub(super) fn uuidv7_pk(manager: &SchemaManager<'_>) -> ColumnDef {
    let mut col = ColumnDef::new(Alias::new("id"));
    if manager.get_database_backend() == DbBackend::Sqlite {
        col.blob();
    } else {
        col.uuid();
    }
    col.not_null().primary_key().clone()
}

/// `created_at TIMESTAMP NOT NULL` on PostgreSQL and `TEXT NOT NULL` on SQLite, without a default.
///
/// SQLite stores `TEXT` because the `timestamp_with_time_zone_text` type `sea-query` emits is not a SQLite type.
/// The application stamps it (see `crate::db::stamped_created_at`).
pub(super) fn created_at(manager: &SchemaManager<'_>) -> ColumnDef {
    let mut col = ColumnDef::new(Alias::new("created_at"));
    if manager.get_database_backend() == DbBackend::Sqlite {
        col.text();
    } else {
        col.timestamp_with_time_zone();
    }
    col.not_null().clone()
}

/// `created_at` plus `updated_at`, both stamped by the application.
pub(super) fn timestamps(manager: &SchemaManager<'_>) -> [ColumnDef; 2] {
    let mut ua = ColumnDef::new(Alias::new("updated_at"));
    if manager.get_database_backend() == DbBackend::Sqlite {
        ua.text();
    } else {
        ua.timestamp_with_time_zone();
    }
    [created_at(manager), ua.not_null().clone()]
}

/// Enables RLS and installs a single-column-equality policy in one raw-SQL round trip.
///
/// The policy is `column = current_setting(setting)::uuid`.
/// Strict mode raises when the setting is missing.
/// Lenient mode (`lenient => true`) reads a missing setting as NULL, which matches nothing.
///
/// Raw SQL is needed because `SeaORM`'s schema builder has no RLS support.
/// All parameters are migration constants.
///
/// No-ops on SQLite, which is single-tenant.
pub(super) async fn enable_rls_with_policy(
    manager: &SchemaManager<'_>,
    table: &str,
    policy_name: &str,
    column: &str,
    setting: &str,
    lenient: bool,
) -> Result<(), DbErr> {
    if manager.get_database_backend() == DbBackend::Sqlite {
        return Ok(());
    }
    let condition = if lenient {
        format!("{column} = NULLIF(current_setting('{setting}', true), '')::uuid")
    } else {
        format!("{column} = current_setting('{setting}')::uuid")
    };
    manager
        .get_connection()
        .execute_unprepared(&format!(
            "ALTER TABLE {table} ENABLE ROW LEVEL SECURITY;
         CREATE POLICY {policy_name} ON {table} USING ({condition});"
        ))
        .await?;
    Ok(())
}

/// A single explicit per-table GRANT to `yorishiro_app`.
///
/// There is deliberately no schema-wide `GRANT ... ON ALL TABLES`, so a table stays ungranted unless a call names it.
///
/// Raw SQL is needed because `SeaORM` has no GRANT builder.
/// No-ops on SQLite, which has no `yorishiro_app` role.
pub(super) async fn grant(
    manager: &SchemaManager<'_>,
    privileges: &str,
    table: &str,
) -> Result<(), DbErr> {
    if manager.get_database_backend() == DbBackend::Sqlite {
        return Ok(());
    }
    manager
        .get_connection()
        .execute_unprepared(&format!("GRANT {privileges} ON {table} TO yorishiro_app;"))
        .await?;
    Ok(())
}

/// Creates `table`, with each `(constraint_name, expr)` in `checks` applied as a table-level CHECK.
///
/// Named constraints are emitted inline by `SeaQuery` on both backends.
/// The `expr` values are CHECK constraint expressions (hardcoded migration constants), not user input.
pub(super) async fn create_table_with_checks(
    manager: &SchemaManager<'_>,
    _table_name: &str,
    mut table: TableCreateStatement,
    checks: &[(&str, &str)],
) -> Result<(), DbErr> {
    for (name, expr) in checks {
        table.check((Alias::new(*name), Expr::cust(String::from(*expr))));
    }
    manager.create_table(table).await?;
    Ok(())
}

/// Runs `sql` only on Postgres; a no-op on SQLite.
///
/// For statements with no SQLite equivalent: roles, column-level GRANTs, `plpgsql` functions and triggers, GIN, HNSW and trigram indexes, partitions and pgvector DDL.
/// A CHECK that SQLite can express belongs in the table definition instead.
pub(super) async fn pg_only(manager: &SchemaManager<'_>, sql: &str) -> Result<(), DbErr> {
    if manager.get_database_backend() == DbBackend::Sqlite {
        return Ok(());
    }
    manager.get_connection().execute_unprepared(sql).await?;
    Ok(())
}

/// Runs `sql` only on SQLite; a no-op on Postgres.
///
/// For the SQLite counterpart of a `pg_only` statement, such as the fork integrity triggers or the FTS5 table.
pub(super) async fn sqlite_only(manager: &SchemaManager<'_>, sql: &str) -> Result<(), DbErr> {
    if manager.get_database_backend() != DbBackend::Sqlite {
        return Ok(());
    }
    manager.get_connection().execute_unprepared(sql).await?;
    Ok(())
}

/// A UUID column: `BLOB` on SQLite, `UUID` on PostgreSQL.
///
/// `SeaORM` binds `Uuid` as a 16-byte BLOB, so the SQLite column is BLOB too.
pub(super) fn uuid_col(manager: &SchemaManager<'_>, name: Alias) -> ColumnDef {
    let mut col = ColumnDef::new(name);
    if manager.get_database_backend() == DbBackend::Sqlite {
        col.blob();
    } else {
        col.uuid();
    }
    col.clone()
}

/// A JSON column: `TEXT` on SQLite, `JSONB` on PostgreSQL.
///
/// SQLite stores `TEXT` because the `jsonb_text` type `sea-query` emits is not a SQLite type.
pub(super) fn json_col(manager: &SchemaManager<'_>, name: Alias) -> ColumnDef {
    let mut col = ColumnDef::new(name);
    if manager.get_database_backend() == DbBackend::Sqlite {
        col.text();
    } else {
        col.json_binary();
    }
    col.clone()
}

/// A `TIMESTAMPTZ` column: `TEXT` on SQLite, `TIMESTAMPTZ` on PostgreSQL.
///
/// SQLite stores `TEXT` for the same reason as `created_at`.
pub(super) fn ts_tz_col(manager: &SchemaManager<'_>, name: Alias) -> ColumnDef {
    let mut col = ColumnDef::new(name);
    if manager.get_database_backend() == DbBackend::Sqlite {
        col.text();
    } else {
        col.timestamp_with_time_zone();
    }
    col.clone()
}

/// Picks the value for the running backend, so a migration never branches on it for a single value.
pub(super) fn by_backend<T>(manager: &SchemaManager<'_>, postgres: T, sqlite: T) -> T {
    if manager.get_database_backend() == DbBackend::Sqlite {
        sqlite
    } else {
        postgres
    }
}
