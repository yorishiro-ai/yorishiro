use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::repositories::entities;
use crate::repositories::relations;
use crate::repositories::schemas;

pub use crate::models::export::*;

/// Fetches every schema (all versions, including archived), entity, and relation for the
/// workspace, for a full-workspace data export. Schemas come first so a reader can resolve the
/// entity_types/relation_types that the entities and relations after them reference.
pub async fn export_all(
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<Vec<ExportRecord>, YorishiroError> {
    let mut records = Vec::new();
    records.extend(
        schemas::export_all(conn, workspace_id)
            .await?
            .into_iter()
            .map(ExportRecord::Schema),
    );
    records.extend(
        entities::export_all(conn, workspace_id)
            .await?
            .into_iter()
            .map(ExportRecord::Entity),
    );
    records.extend(
        relations::export_all(conn, workspace_id)
            .await?
            .into_iter()
            .map(ExportRecord::Relation),
    );
    Ok(records)
}

#[cfg(test)]
#[path = "../../tests/repositories/export.rs"]
mod tests;
