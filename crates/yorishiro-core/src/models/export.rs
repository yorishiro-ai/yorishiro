use serde::{Deserialize, Serialize};
use serde_json::Value;
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
pub async fn export_all<C>(
    conn: &mut C,
    workspace_id: Uuid,
) -> Result<Vec<ExportRecord>, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    EntityRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
    RelationRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
    // Transcribed from `schemas::export_all`'s private `SchemaRow` bound; see `entities::update`'s where clause for why this can't be named directly.
    Uuid: for<'q> sqlx::Encode<'q, C::Db> + for<'r> sqlx::Decode<'r, C::Db> + sqlx::Type<C::Db>,
    Option<Uuid>: for<'r> sqlx::Decode<'r, C::Db> + sqlx::Type<C::Db>,
    String: for<'r> sqlx::Decode<'r, C::Db> + sqlx::Type<C::Db>,
    i32: for<'r> sqlx::Decode<'r, C::Db> + sqlx::Type<C::Db>,
    Value: for<'r> sqlx::Decode<'r, C::Db> + sqlx::Type<C::Db>,
    Option<Value>: for<'r> sqlx::Decode<'r, C::Db> + sqlx::Type<C::Db>,
    chrono::DateTime<chrono::Utc>: for<'r> sqlx::Decode<'r, C::Db> + sqlx::Type<C::Db>,
    for<'a> &'a str: sqlx::ColumnIndex<<C::Db as sqlx::Database>::Row>,
{
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
