//! Enterprise-only persistence on the community `entity_entities` table.

pub mod infer_fill;

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::Expr;
use serde_json::Value;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::entity_entities::Column;
use crate::models::entity_entities::{Entity, EntityRecord};

/// Updates an entity only when it still has the timestamp read by the caller.
/// PostgreSQL callers also lock the row before reading it, while SQLite's transaction-level
/// single-writer rule protects this transaction-scoped confirmation path.
pub(crate) async fn update_if_unchanged(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    existing: &EntityRecord,
    data: Value,
    schema_id: Uuid,
    schema_version: i32,
    updated_by: Option<Uuid>,
) -> Result<bool, YorishiroError> {
    let mut update = Entity::update_many()
        .col_expr(Column::Data, Expr::value(data))
        .col_expr(Column::SchemaId, Expr::value(schema_id))
        .col_expr(Column::SchemaVersion, Expr::value(schema_version))
        .col_expr(Column::UpdatedBy, Expr::value(updated_by))
        .col_expr(Column::UpdatedAt, Expr::value(crate::data::time::now()))
        .col_expr(
            Column::EmbeddingSyncToken,
            Expr::value(crate::models::entity_entities::new_sync_token()),
        )
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(existing.id));
    if crate::db::is_postgres(conn) {
        update = update.filter(Column::UpdatedAt.eq(existing.updated_at));
    }
    let result = update.exec(conn).await.internal()?;
    if result.rows_affected == 1 {
        crate::models::entity_entities::touch_content_revision(conn, workspace_id)
            .await
            .internal()?;
        crate::models::entity_entities::resync_full_text(conn, existing.id)
            .await
            .internal()?;
    }
    Ok(result.rows_affected == 1)
}
