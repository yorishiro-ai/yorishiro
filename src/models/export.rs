use sea_orm::ConnectionTrait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::content_entities::{self, EntityRecord};
use crate::models::content_relations::{self, RelationRecord};
use crate::models::content_schemas::{self, SchemaRecord};

/// One line of a JSONL export: a tagged union so schema/entity/relation records can be told
/// apart on read-back without a separate line-position convention.
/// `Deserialize` is derived too so `models::import::import_jsonl` can read the same shape back
/// in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "record", rename_all = "snake_case")]
pub enum ExportRecord {
    Schema(SchemaRecord),
    Entity(EntityRecord),
    Relation(RelationRecord),
}

/// Fetches every schema (all versions, including archived), entity, and relation for the
/// workspace, for a full-workspace data export.
/// Schemas come first so a reader can resolve the entity_types/relation_types that the entities
/// and relations after them reference.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn export_all(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<ExportRecord>, YorishiroError> {
    let mut records = Vec::new();
    records.extend(
        content_schemas::export_all(conn, workspace_id)
            .await?
            .into_iter()
            .map(ExportRecord::Schema),
    );
    records.extend(
        content_entities::export_all(conn, workspace_id)
            .await?
            .into_iter()
            .map(ExportRecord::Entity),
    );
    records.extend(
        content_relations::export_all(conn, workspace_id)
            .await?
            .into_iter()
            .map(ExportRecord::Relation),
    );
    Ok(records)
}
