//! Reusable column/statement builders shared across migration files.
//!
//! `loco_rs::schema::ColType` has no variant that is both a primary key and carries a custom default expression, so every table's `id` column goes through `sea_query::ColumnDef` directly.
use sea_orm_migration::prelude::*;

/// `id UUID PRIMARY KEY DEFAULT uuidv7()`.
pub fn uuidv7_pk() -> ColumnDef {
    ColumnDef::new(Alias::new("id"))
        .uuid()
        .not_null()
        .primary_key()
        .default(Expr::cust("uuidv7()"))
        .to_owned()
}

/// `created_at TIMESTAMPTZ NOT NULL DEFAULT now()`.
pub fn created_at() -> ColumnDef {
    ColumnDef::new(Alias::new("created_at"))
        .timestamp_with_time_zone()
        .not_null()
        .default(Expr::current_timestamp())
        .to_owned()
}

/// `created_at` plus `updated_at`, both `TIMESTAMPTZ NOT NULL DEFAULT now()`.
pub fn timestamps() -> [ColumnDef; 2] {
    [
        created_at(),
        ColumnDef::new(Alias::new("updated_at"))
            .timestamp_with_time_zone()
            .not_null()
            .default(Expr::current_timestamp())
            .to_owned(),
    ]
}

/// Enables RLS and installs a single-column-equality policy in one raw-SQL round trip.
///
/// `column = current_setting(setting)::uuid`, either strict (missing setting raises) or lenient (`lenient => true`, missing setting reads as NULL, matching nothing): strict for tables `yorishiro_app` always reaches with both GUCs set, lenient for tables the control-plane pool also reaches without naming a workspace.
pub async fn enable_rls_with_policy(
    db: &SchemaManagerConnection<'_>,
    table: &str,
    policy_name: &str,
    column: &str,
    setting: &str,
    lenient: bool,
) -> Result<(), DbErr> {
    let condition = if lenient {
        format!("{column} = NULLIF(current_setting('{setting}', true), '')::uuid")
    } else {
        format!("{column} = current_setting('{setting}')::uuid")
    };
    db.execute_unprepared(&format!(
        "ALTER TABLE {table} ENABLE ROW LEVEL SECURITY;
         CREATE POLICY {policy_name} ON {table} USING ({condition});"
    ))
    .await?;
    Ok(())
}

/// A single explicit per-table GRANT to `yorishiro_app`.
///
/// Deliberately never a schema-wide `GRANT ... ON ALL TABLES IN SCHEMA public`: that would sweep in the tables that must stay ungranted (`identity_tenants`, `identity_users`, `identity_tenant_memberships`, `identity_invites`, `identity_templates`, `identity_workspace_llm_keys`).
/// Every grant is named here, one call per table, so an ungranted table is ungranted because no call exists for it, not because a wildcard missed it.
pub async fn grant(
    db: &SchemaManagerConnection<'_>,
    privileges: &str,
    table: &str,
) -> Result<(), DbErr> {
    db.execute_unprepared(&format!("GRANT {privileges} ON {table} TO yorishiro_app;"))
        .await?;
    Ok(())
}
