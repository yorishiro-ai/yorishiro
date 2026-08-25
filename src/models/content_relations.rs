use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, FromQueryResult, QueryOrder, QuerySelect, SqlErr, Statement};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

pub use super::_entities::content_relations::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, ValidationDetail, YorishiroError};
use crate::models::content_entities::{self, EntityRecord};

pub type ContentRelations = Entity;

/// The generated `Model` already matches this API's response shape, so this is an alias rather than a distinct struct.
pub type RelationRecord = Model;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    /// `id` has a `uuidv7()` column default on PostgreSQL and no default on SQLite; see `crate::db::sqlite_generated_id`.
    async fn before_save<C>(self, db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::sqlite_generated_id(db, this.id);
        Ok(this)
    }
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}

/// The state a relation is created in, and the only one traversal follows.
pub const RELATION_STATUS_ACTIVE: &str = "active";

/// Every state a relation may hold, matching the check constraint on `content_relations`.
pub const RELATION_STATUSES: [&str; 3] = ["active", "deprecated", "archived"];

/// Whether `status` names a state a relation may hold.
/// Callers validate before writing so an unknown value is a 422 naming the field, not a constraint violation surfacing as a 500.
pub fn is_valid_relation_status(status: &str) -> bool {
    RELATION_STATUSES.contains(&status)
}

pub struct CreateRelationInput {
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    pub properties: Value,
}

#[derive(Default)]
pub struct ListRelationsQuery {
    pub source_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub relation_type: Option<String>,
    /// Restricts the listing to one state.
    /// `None` lists every state, so a caller that does not pass `status` sees deprecated and archived relations along with every other state.
    pub status: Option<String>,
    pub page: super::pagination::ListParams,
}

/// Validates that `relation_type` doesn't conflict with the source/target entity_types.
/// The metaschema definition is resolved against the schema the source entity was actually created with (the row's `schema_id`), so existing relationships between entities don't silently break even as the active schema evolves.
async fn validate_relation_type(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    source: &EntityRecord,
    target: &EntityRecord,
    relation_type: &str,
) -> Result<(), YorishiroError> {
    let schema =
        crate::models::content_schemas::get_by_id(conn, workspace_id, source.schema_id).await?;

    let relation_def = schema
        .definition
        .relation_types
        .get(relation_type)
        .ok_or_else(|| {
            YorishiroError::not_found(format!(
                "relation_type '{relation_type}' is not defined in schema '{}'",
                schema.definition.name
            ))
        })?;

    if relation_def.source != source.entity_type || relation_def.target != target.entity_type {
        return Err(YorishiroError::RelationTypeMismatch {
            message: format!(
                "relation_type '{relation_type}' expects source='{}' target='{}', \
                 but got source='{}' target='{}'",
                relation_def.source, relation_def.target, source.entity_type, target.entity_type
            ),
        });
    }

    Ok(())
}

/// Creates a new relation: verifies both the source and target entities exist and that `relation_type` matches the metaschema's source/target constraint, then persists it.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn create(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    input: CreateRelationInput,
) -> Result<RelationRecord, YorishiroError> {
    let source = content_entities::get(conn, workspace_id, input.source_id).await?;
    let target = content_entities::get(conn, workspace_id, input.target_id).await?;
    validate_relation_type(conn, workspace_id, &source, &target, &input.relation_type).await?;

    let properties = if input.properties.is_null() {
        json!({})
    } else {
        input.properties
    };

    let active = ActiveModel {
        workspace_id: ActiveValue::Set(workspace_id),
        source_id: ActiveValue::Set(input.source_id),
        target_id: ActiveValue::Set(input.target_id),
        relation_type: ActiveValue::Set(input.relation_type.clone()),
        properties: ActiveValue::Set(properties),
        ..Default::default()
    };
    active.insert(conn).await.map_err(|err| {
        if matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
            YorishiroError::Conflict {
                message: format!(
                    "relation '{}' between '{}' and '{}' already exists",
                    input.relation_type, input.source_id, input.target_id
                ),
            }
        } else if matches!(
            err.sql_err(),
            Some(SqlErr::ForeignKeyConstraintViolation(_))
        ) {
            // A TOCTOU window between checking source/target existence and the INSERT, during which another transaction could delete the entity.
            // Treated as NotFound, same as the upfront check.
            YorishiroError::not_found(format!(
                "source '{}' or target '{}' no longer exists",
                input.source_id, input.target_id
            ))
        } else {
            YorishiroError::Internal(err.into())
        }
    })
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn get(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<RelationRecord, YorishiroError> {
    use super::_entities::content_relations::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(id))
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("relation '{id}' was not found")))
}

/// Moves a relation to another state.
/// Retiring a relation this way keeps the record that it existed, which deleting it does not; traversal stops following it either way.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn set_status(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
    status: &str,
) -> Result<RelationRecord, YorishiroError> {
    if !is_valid_relation_status(status) {
        return Err(YorishiroError::ValidationFailed {
            message: format!("'{status}' is not a relation status"),
            details: vec![ValidationDetail {
                field: "/status".to_string(),
                problem: format!("expected one of {}", RELATION_STATUSES.join(", ")),
            }],
            hint: format!(
                "use one of {}: traversal follows '{RELATION_STATUS_ACTIVE}' only",
                RELATION_STATUSES.join(", ")
            ),
        });
    }

    let existing = get(conn, workspace_id, id).await?;

    let active = ActiveModel {
        id: ActiveValue::Unchanged(existing.id),
        status: ActiveValue::Set(status.to_string()),
        ..Default::default()
    };
    // A concurrent delete between `get` above and this update surfaces as `DbErr::RecordNotUpdated`, not a row.
    // Map it to the same 404 the upfront `get` would have returned had it lost the race instead, rather than letting `.internal()` turn it into a 500.
    active.update(conn).await.map_err(|err| match err {
        DbErr::RecordNotUpdated => {
            YorishiroError::not_found(format!("relation '{id}' was not found"))
        }
        err => YorishiroError::Internal(err.into()),
    })
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn delete(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<(), YorishiroError> {
    use super::_entities::content_relations::Column;

    let result = Entity::delete_many()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(id))
        .exec(conn)
        .await
        .internal()?;

    if result.rows_affected == 0 {
        Err(YorishiroError::not_found(format!(
            "relation '{id}' was not found"
        )))
    } else {
        Ok(())
    }
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn list(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    query: ListRelationsQuery,
) -> Result<Vec<RelationRecord>, YorishiroError> {
    use super::_entities::content_relations::Column;

    let mut select = Entity::find().filter(Column::WorkspaceId.eq(workspace_id));
    if let Some(source_id) = query.source_id {
        select = select.filter(Column::SourceId.eq(source_id));
    }
    if let Some(target_id) = query.target_id {
        select = select.filter(Column::TargetId.eq(target_id));
    }
    if let Some(relation_type) = query.relation_type {
        select = select.filter(Column::RelationType.eq(relation_type));
    }
    if let Some(status) = query.status {
        select = select.filter(Column::Status.eq(status));
    }

    select
        .order_by_desc(Column::CreatedAt)
        .limit(query.page.limit() as u64)
        .offset(query.page.offset() as u64)
        .all(conn)
        .await
        .internal()
}

/// Counts how many relations a workspace holds, for workspace-detail summaries.
pub async fn count(conn: &impl ConnectionTrait, workspace_id: Uuid) -> Result<i64, YorishiroError> {
    use super::_entities::content_relations::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .count(conn)
        .await
        .internal()
        .map(|n| n as i64)
}

/// Fetches every relation for the workspace, with no pagination limit, for a full-workspace data export.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn export_all(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<RelationRecord>, YorishiroError> {
    use super::_entities::content_relations::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(Column::CreatedAt)
        .all(conn)
        .await
        .internal()
}

pub const DEFAULT_NEIGHBORS_LIMIT: i64 = 20;

/// A relation together with the entity on the other end of it, relative to the entity `neighbors_batch` was called for.
/// `direction` is `"out"` when the queried entity is the relation's source (the neighbor is the target) and `"in"` when it's the target (the neighbor is the source).
#[derive(Debug, Clone, Serialize)]
pub struct Neighbor {
    pub relation_id: Uuid,
    pub relation_type: String,
    pub direction: String,
    pub properties: Value,
    pub entity: EntityRecord,
}

#[derive(Debug, FromQueryResult)]
struct BatchNeighborRow {
    pivot_id: Uuid,
    relation_id: Uuid,
    relation_type: String,
    direction: String,
    properties: Value,
    // Only used to drive the SQL-level ORDER BY; not read on the Rust side.
    #[allow(dead_code)]
    relation_created_at: chrono::DateTime<chrono::FixedOffset>,
    entity_id: Uuid,
    entity_workspace_id: Uuid,
    entity_schema_id: Uuid,
    entity_schema_version: i32,
    entity_type: String,
    entity_data: Value,
    entity_created_at: chrono::DateTime<chrono::FixedOffset>,
    entity_updated_at: chrono::DateTime<chrono::FixedOffset>,
    entity_created_by: Option<Uuid>,
    entity_updated_by: Option<Uuid>,
}

impl BatchNeighborRow {
    fn into_neighbor(self) -> Neighbor {
        Neighbor {
            relation_id: self.relation_id,
            relation_type: self.relation_type,
            direction: self.direction,
            properties: self.properties,
            entity: EntityRecord {
                id: self.entity_id,
                workspace_id: self.entity_workspace_id,
                schema_id: self.entity_schema_id,
                schema_version: self.entity_schema_version,
                entity_type: self.entity_type,
                data: self.entity_data,
                created_at: self.entity_created_at.into(),
                updated_at: self.entity_updated_at.into(),
                created_by: self.entity_created_by,
                updated_by: self.entity_updated_by,
            },
        }
    }
}

/// Batched neighbor lookup: finds up to `limit` neighbors of every id in `pivot_ids` in one round trip instead of one call per id, via `CROSS JOIN LATERAL` so each pivot still gets its own `limit`-bounded result (a plain `WHERE source_id = ANY(...) LIMIT n` would apply `limit` across the whole batch instead of per pivot, which is not the same query).
///
/// Returns a map from pivot id to its neighbors; a pivot with no relations at all is absent from the map rather than present with an empty vec.
/// A duplicate id in `pivot_ids` contributes only once (deduped before querying).
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn neighbors_batch(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    pivot_ids: &[Uuid],
    limit: i64,
) -> Result<std::collections::HashMap<Uuid, Vec<Neighbor>>, YorishiroError> {
    let limit = limit.clamp(1, 200);
    let pivot_ids: Vec<Uuid> = pivot_ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    if pivot_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    // The lateral's own ORDER BY/LIMIT already picks the right *set* of up-to-`limit` rows per pivot; the outer ORDER BY guarantees those rows come back to Rust in per-pivot, most-recent-first order too.
    // `CROSS JOIN LATERAL` doesn't otherwise promise the driving join order is preserved across pivots, and `recall_context`'s truncation check relies on it.
    let rows = BatchNeighborRow::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT pivot.id AS pivot_id, n.relation_id, n.relation_type, n.direction, \
                n.properties, n.relation_created_at, n.entity_id, n.entity_workspace_id, \
                n.entity_schema_id, n.entity_schema_version, n.entity_type, n.entity_data, \
                n.entity_created_at, n.entity_updated_at, n.entity_created_by, \
                n.entity_updated_by \
         FROM unnest($2::uuid[]) AS pivot(id) \
         CROSS JOIN LATERAL ( \
             SELECT r.id AS relation_id, r.relation_type, 'out' AS direction, r.properties, \
                    r.created_at AS relation_created_at, \
                    e.id AS entity_id, e.workspace_id AS entity_workspace_id, \
                    e.schema_id AS entity_schema_id, e.schema_version AS entity_schema_version, \
                    e.entity_type, e.data AS entity_data, e.created_at AS entity_created_at, \
                    e.updated_at AS entity_updated_at, e.created_by AS entity_created_by, \
                    e.updated_by AS entity_updated_by \
             FROM content_relations r \
             JOIN content_entities e ON e.id = r.target_id AND e.workspace_id = r.workspace_id \
             WHERE r.workspace_id = $1 AND r.source_id = pivot.id AND r.status = 'active' \
             UNION ALL \
             SELECT r.id, r.relation_type, 'in' AS direction, r.properties, r.created_at, \
                    e.id, e.workspace_id, e.schema_id, e.schema_version, e.entity_type, e.data, \
                    e.created_at, e.updated_at, e.created_by, e.updated_by \
             FROM content_relations r \
             JOIN content_entities e ON e.id = r.source_id AND e.workspace_id = r.workspace_id \
             WHERE r.workspace_id = $1 AND r.target_id = pivot.id AND r.status = 'active' \
             ORDER BY relation_created_at DESC \
             LIMIT $3 \
         ) AS n \
         ORDER BY pivot.id, n.relation_created_at DESC",
        [workspace_id.into(), pivot_ids.into(), limit.into()],
    ))
    .all(conn)
    .await
    .internal()?;

    let mut by_pivot: std::collections::HashMap<Uuid, Vec<Neighbor>> =
        std::collections::HashMap::new();
    for row in rows {
        by_pivot
            .entry(row.pivot_id)
            .or_default()
            .push(row.into_neighbor());
    }

    Ok(by_pivot)
}
