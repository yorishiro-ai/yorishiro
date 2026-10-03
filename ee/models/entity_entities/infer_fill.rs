//! Finding the entities `infer_fill` should consider.
//!
//! Finding the entities eligible for infer-fill remains separate from proposal persistence.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::entity_entities as content_entities_entity;
use crate::models::_entities::schema_schemas;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect};
use serde_json::Value;
use uuid::Uuid;

/// One entity `infer_fill` should consider: still on some earlier version of `name`, not yet on `active_schema_id`.
pub(crate) struct OutdatedEntity {
    pub(crate) id: Uuid,
    pub(crate) entity_type: String,
    pub(crate) data: Value,
}

/// Every entity in `workspace_id` still on a version of the schema named `name` other than `active_schema_id`.
/// The set `infer_fill` walks: an entity already on the active version has nothing the schema says is missing.
///
/// `schema_schemas` rows sharing `(workspace_id, name)` are versions of the same logical schema, so this first
/// collects every version's id, then filters `entity_entities` by membership in that set: an entity-API
/// equivalent of the join, since neither table needs a column the other doesn't already expose.
pub(crate) async fn entities_on_outdated_schema(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    name: &str,
    active_schema_id: Uuid,
) -> Result<Vec<OutdatedEntity>, YorishiroError> {
    let schema_ids: Vec<Uuid> = schema_schemas::Entity::find()
        .filter(schema_schemas::Column::WorkspaceId.eq(workspace_id))
        .filter(schema_schemas::Column::Name.eq(name))
        .select_only()
        .column(schema_schemas::Column::Id)
        .into_tuple()
        .all(conn)
        .await
        .internal()?;

    let rows = content_entities_entity::Entity::find()
        .filter(content_entities_entity::Column::WorkspaceId.eq(workspace_id))
        .filter(content_entities_entity::Column::SchemaId.is_in(schema_ids))
        .filter(content_entities_entity::Column::SchemaId.ne(active_schema_id))
        .all(conn)
        .await
        .internal()?;

    Ok(rows
        .into_iter()
        .map(|row| OutdatedEntity {
            id: row.id,
            entity_type: row.entity_type,
            data: row.data,
        })
        .collect())
}
