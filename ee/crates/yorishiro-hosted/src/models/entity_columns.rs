//! Which columns the Entities table shows, per workspace and entity type.
//!
//! Scoped to the workspace rather than the user, so everyone looking at a workspace sees the same table.
//! A per-user override belongs in this table as a second nullable column, not in a second table.

use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use yorishiro_core::error::{ResultExt, YorishiroError};
use yorishiro_core::models::_entities::content_entity_column_preferences::{
    ActiveModel, Column, Entity,
};
use yorishiro_core::models::pagination::ListParams;

/// How many columns a workspace may turn on at once.
///
/// Not a storage limit: a table wide enough to scroll horizontally stops being a table, and a schema with sixty fields would otherwise let one click produce one.
pub const MAX_VISIBLE_COLUMNS: usize = 12;

/// The visible columns for one entity type, in display order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnPreference {
    pub entity_type: String,
    /// Field names from the schema, in the order they are displayed.
    /// A name the schema no longer defines stays here and is skipped when rendering, so a schema change does not have to know about display settings.
    pub columns: Vec<String>,
}

fn to_preference(
    model: yorishiro_core::models::_entities::content_entity_column_preferences::Model,
) -> ColumnPreference {
    ColumnPreference {
        entity_type: model.entity_type,
        columns: serde_json::from_value(model.columns).unwrap_or_default(),
    }
}

/// Reads the stored preference for one entity type.
///
/// `None` means the workspace has never chosen, which is different from having chosen nothing: the caller falls back to the schema's own first few fields rather than rendering a table with no columns.
pub async fn get(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_type: &str,
) -> Result<Option<ColumnPreference>, YorishiroError> {
    let row = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::EntityType.eq(entity_type))
        .one(conn)
        .await
        .internal()?;

    Ok(row.map(to_preference))
}

/// Every stored preference in the workspace, so a caller can switch entity types without a round trip each time.
pub async fn list(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    page: ListParams,
) -> Result<Vec<ColumnPreference>, YorishiroError> {
    let rows = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(Column::EntityType)
        .limit(page.limit() as u64)
        .offset(page.offset() as u64)
        .all(conn)
        .await
        .internal()?;

    Ok(rows.into_iter().map(to_preference).collect())
}

/// Stores the choice, replacing whatever was there.
///
/// `ON CONFLICT` rather than a read-then-write: two tabs saving at once would otherwise both see no row and both insert, and the unique constraint would turn the loser into a 500.
pub async fn set(
    conn: &impl ConnectionTrait,
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

    // A duplicate would render the same field twice and make a drag-to-reorder UI ambiguous.
    let mut seen = std::collections::HashSet::new();
    if let Some(dup) = columns.iter().find(|c| !seen.insert(*c)) {
        return Err(YorishiroError::ValidationFailed {
            message: format!("column '{dup}' is listed more than once"),
            details: vec![],
            hint: "each field can appear at most once".into(),
        });
    }

    let encoded = serde_json::to_value(columns).internal()?;

    let active = ActiveModel {
        workspace_id: ActiveValue::Set(workspace_id),
        entity_type: ActiveValue::Set(entity_type.to_string()),
        columns: ActiveValue::Set(encoded),
        updated_at: ActiveValue::Set(chrono::Utc::now().into()),
        ..Default::default()
    };
    Entity::insert(active)
        .on_conflict(
            OnConflict::columns([Column::WorkspaceId, Column::EntityType])
                .update_columns([Column::Columns, Column::UpdatedAt])
                .to_owned(),
        )
        .exec(conn)
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
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_type: &str,
) -> Result<(), YorishiroError> {
    Entity::delete_many()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::EntityType.eq(entity_type))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}
