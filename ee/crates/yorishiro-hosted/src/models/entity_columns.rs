//! Which columns the Entities table shows, per workspace and entity type.
//!
//! The create form has always been schema-driven; the table beside it showed a fixed four columns.
//! This is what lets the table follow the schema too, without guessing which fields matter: the workspace says so.
//!
//! Scoped to the workspace rather than the user, so everyone looking at a workspace sees the same table.
//! A per-user override belongs in this table as a second nullable column, not in a second table.

use sea_query::{Alias, Expr, Iden, OnConflict, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use yorishiro_core::{ResultExt, YorishiroError};

#[derive(Iden)]
enum EntityColumnPreferences {
    Table,
    WorkspaceId,
    EntityType,
    Columns,
    UpdatedAt,
}

/// How many columns a workspace may turn on at once.
///
/// Not a storage limit: a table wide enough to scroll horizontally stops being a table, and a schema with sixty fields would otherwise let one click produce one.
pub const MAX_VISIBLE_COLUMNS: usize = 12;

/// The visible columns for one entity type, in display order.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ColumnPreference {
    pub entity_type: String,
    /// Field names from the schema, in the order they are displayed.
    /// A name the schema no longer defines stays here and is skipped when rendering, so a schema change does not have to know about display settings.
    pub columns: Vec<String>,
}

/// Reads the stored preference for one entity type.
///
/// `None` means the workspace has never chosen, which is different from having chosen nothing: the caller falls back to the schema's own first few fields rather than rendering a table with no columns.
pub async fn get(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    entity_type: &str,
) -> Result<Option<ColumnPreference>, YorishiroError> {
    let (sql, values) = Query::select()
        .column(EntityColumnPreferences::Columns)
        .from((Alias::new("content"), EntityColumnPreferences::Table))
        .and_where(Expr::col(EntityColumnPreferences::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(EntityColumnPreferences::EntityType).eq(entity_type))
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<(serde_json::Value,)> = sqlx::query_as_with(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .internal()?;

    Ok(row.map(|(columns,)| ColumnPreference {
        entity_type: entity_type.to_string(),
        columns: serde_json::from_value(columns).unwrap_or_default(),
    }))
}

/// Every stored preference in the workspace, so the Entities page can switch entity types without a round trip each time.
pub async fn list(
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<Vec<ColumnPreference>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns([
            EntityColumnPreferences::EntityType,
            EntityColumnPreferences::Columns,
        ])
        .from((Alias::new("content"), EntityColumnPreferences::Table))
        .and_where(Expr::col(EntityColumnPreferences::WorkspaceId).eq(workspace_id))
        .order_by(EntityColumnPreferences::EntityType, sea_query::Order::Asc)
        .build_sqlx(PostgresQueryBuilder);

    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as_with(&sql, values)
        .fetch_all(&mut *conn)
        .await
        .internal()?;

    Ok(rows
        .into_iter()
        .map(|(entity_type, columns)| ColumnPreference {
            entity_type,
            columns: serde_json::from_value(columns).unwrap_or_default(),
        })
        .collect())
}

/// Stores the choice, replacing whatever was there.
///
/// `ON CONFLICT` rather than a read-then-write: two tabs saving at once would otherwise both see no row and both insert, and the unique constraint would turn the loser into a 500.
pub async fn set(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    entity_type: &str,
    columns: &[String],
) -> Result<ColumnPreference, YorishiroError> {
    if columns.len() > MAX_VISIBLE_COLUMNS {
        return Err(YorishiroError::ValidationFailed {
            message: format!(
                "at most {MAX_VISIBLE_COLUMNS} columns can be shown at once, got {}",
                columns.len()
            ),
            details: vec![],
            hint: "turn some columns off before turning others on".into(),
        });
    }

    // A duplicate would render the same field twice and make the drag-to-reorder ambiguous.
    let mut seen = std::collections::HashSet::new();
    if let Some(dup) = columns.iter().find(|c| !seen.insert(*c)) {
        return Err(YorishiroError::ValidationFailed {
            message: format!("column '{dup}' is listed more than once"),
            details: vec![],
            hint: "each field can appear at most once".into(),
        });
    }

    let encoded = serde_json::to_value(columns).internal()?;

    let (sql, values) = Query::insert()
        .into_table((Alias::new("content"), EntityColumnPreferences::Table))
        .columns([
            EntityColumnPreferences::WorkspaceId,
            EntityColumnPreferences::EntityType,
            EntityColumnPreferences::Columns,
        ])
        .values_panic([workspace_id.into(), entity_type.into(), encoded.into()])
        .on_conflict(
            OnConflict::columns([
                EntityColumnPreferences::WorkspaceId,
                EntityColumnPreferences::EntityType,
            ])
            .update_columns([EntityColumnPreferences::Columns])
            .value(EntityColumnPreferences::UpdatedAt, Expr::current_timestamp())
            .to_owned(),
        )
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_with(&sql, values)
        .execute(&mut *conn)
        .await
        .internal()?;

    Ok(ColumnPreference {
        entity_type: entity_type.to_string(),
        columns: columns.to_vec(),
    })
}

/// Drops the choice, so the table goes back to the schema-derived default.
///
/// Deleting the row rather than storing an empty list: an empty list is itself a choice, "show no columns", and a reset has to be distinguishable from it.
pub async fn clear(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    entity_type: &str,
) -> Result<(), YorishiroError> {
    let (sql, values) = Query::delete()
        .from_table((Alias::new("content"), EntityColumnPreferences::Table))
        .and_where(Expr::col(EntityColumnPreferences::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(EntityColumnPreferences::EntityType).eq(entity_type))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_with(&sql, values)
        .execute(&mut *conn)
        .await
        .internal()?;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/models/entity_columns.rs"]
mod tests;
