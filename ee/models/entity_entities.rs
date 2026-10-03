//! Enterprise-only persistence on the community `entity_entities` table.

pub(crate) mod infer_fill;

use chrono::Utc;
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
        .col_expr(Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(existing.id));
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Postgres {
        update = update.filter(Column::UpdatedAt.eq(existing.updated_at));
    }
    let result = update.exec(conn).await.internal()?;
    Ok(result.rows_affected == 1)
}
