use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::entities::{self, EntityRecord};
use crate::models::relations::{self, RelationRecord};
use crate::models::schemas::{self, SchemaRecord};

/// One line of a JSONL export: a tagged union so schema/entity/relation records can be told apart on read-back without a separate line-position convention.
/// `Deserialize` is derived too so `models::import::import_jsonl` can read the same shape back in.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "record", rename_all = "snake_case")]
pub enum ExportRecord {
    Schema(SchemaRecord),
    Entity(EntityRecord),
    Relation(RelationRecord),
}

/// Fetches every schema (all versions, including archived), entity, and relation for the workspace, for a full-workspace data export.
/// Schemas come first so a reader can resolve the entity_types/relation_types that the entities and relations after them reference.
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
#[path = "../../tests/models/export.rs"]
mod tests;
