//! Which columns the Entities table shows, per workspace and entity type.
//!
//! Scoped to the workspace rather than the user, so everyone looking at a workspace sees the same table.
//! A per-user override belongs in this table as a second nullable column, not in a second table.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::entity_column_preferences::{ActiveModel, Column, Entity};
use crate::models::pagination::ListParams;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How many columns a workspace may turn on at once.
///
/// Not a storage limit: a table wide enough to scroll horizontally stops being a table, and a schema with sixty fields would otherwise let one click produce one.
pub const MAX_VISIBLE_COLUMNS: usize = 12;

/// The visible columns for one entity type, in display order.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub(crate) struct ColumnPreference {
    pub(crate) workspace_id: Uuid,
    pub(crate) entity_type: String,
    /// Field names from the schema, in the order they are displayed.
    /// A name the schema no longer defines stays here and is skipped when rendering, so a schema change does not have to know about display settings.
    pub(crate) columns: Vec<String>,
}

fn to_preference(
    model: crate::models::_entities::entity_column_preferences::Model,
) -> ColumnPreference {
    ColumnPreference {
        workspace_id: model.workspace_id,
        entity_type: model.entity_type,
        columns: serde_json::from_value(model.columns).unwrap_or_default(),
    }
}

/// Every stored preference in the workspace, so a caller can switch entity types without a round trip each time.
pub(crate) async fn list(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    page: ListParams,
) -> Result<Vec<ColumnPreference>, YorishiroError> {
    let rows = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(Column::EntityType)
        .limit(page.limit())
        .offset(page.offset())
        .all(conn)
        .await
        .internal()?;

    Ok(rows.into_iter().map(to_preference).collect())
}

pub(crate) async fn list_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::api_keys::AuthContext,
    page: ListParams,
) -> Result<Vec<ColumnPreference>, YorishiroError> {
    use crate::models::iam::{access::read_workspaces, policy::Action};
    let workspaces = read_workspaces(&ctx.db, actor, Action::EntityColumnsGet).await?;
    let txn = crate::db::begin_workspace_list(ctx, actor.tenant_id, &workspaces).await?;
    let records = if let [workspace] = workspaces.as_slice() {
        list(&txn, *workspace, page).await?
    } else {
        Entity::find()
            .filter(Column::WorkspaceId.is_in(workspaces))
            .order_by_asc(Column::EntityType)
            .order_by_asc(Column::WorkspaceId)
            .limit(page.limit())
            .offset(page.offset())
            .all(&txn)
            .await
            .internal()?
            .into_iter()
            .map(to_preference)
            .collect()
    };
    txn.commit().await.internal()?;
    Ok(records)
}

/// Stores the choice, replacing whatever was there.
///
/// `ON CONFLICT` rather than a read-then-write: two tabs saving at once would otherwise both see no row and both insert, and the unique constraint would turn the loser into a 500.
pub(crate) async fn set(
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
        id: crate::db::sqlite_generated_id(conn, ActiveValue::NotSet),
        workspace_id: ActiveValue::Set(workspace_id),
        entity_type: ActiveValue::Set(entity_type.to_string()),
        columns: ActiveValue::Set(encoded),
        updated_at: ActiveValue::Set(crate::data::time::now().into()),
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
        workspace_id,
        entity_type: entity_type.to_string(),
        columns: columns.to_vec(),
    })
}

/// Drops the choice, so the table goes back to the schema-derived default.
///
/// Deleting the row rather than storing an empty list: an empty list is itself a choice, "show no columns", and a reset has to be distinguishable from it.
pub(crate) async fn clear(
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

/// Opens the caller's write session and stores the column preference for an entity type.
///
/// # Errors
/// Returns the session, authorization or store error.
pub(crate) async fn set_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::api_keys::AuthContext,
    entity_type: &str,
    columns: &[String],
) -> Result<ColumnPreference, YorishiroError> {
    crate::models::iam::session::WorkspaceSession::run(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::EntityColumnsUpdate,
        ),
        Some(crate::models::iam::policy::Action::EntityColumnsUpdate),
        async |session| set(session.txn(), session.workspace_id()?, entity_type, columns).await,
    )
    .await
}

/// Opens the caller's write session and clears the column preference for an entity type.
///
/// # Errors
/// Returns the session, authorization or delete error.
pub(crate) async fn clear_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::api_keys::AuthContext,
    entity_type: &str,
) -> Result<(), YorishiroError> {
    crate::models::iam::session::WorkspaceSession::run(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::EntityColumnsDelete,
        ),
        Some(crate::models::iam::policy::Action::EntityColumnsDelete),
        async |session| clear(session.txn(), session.workspace_id()?, entity_type).await,
    )
    .await
}
