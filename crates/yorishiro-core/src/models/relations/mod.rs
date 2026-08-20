use chrono::{DateTime, Utc};
use sea_query::{Asterisk, Expr, Func, Iden, Order, Query};
use sea_query_binder::SqlxBinder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::entities;
use crate::models::entities::EntityRecord;
use crate::models::schemas;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct RelationRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    #[schema(value_type = Object)]
    pub properties: Value,
    /// `active`, `deprecated` or `archived`.
    /// Traversal follows `active` relations only.
    pub status: String,
    pub created_at: DateTime<Utc>,
}

/// The state a relation is created in, and the only one traversal follows.
pub const RELATION_STATUS_ACTIVE: &str = "active";

/// Every state a relation may hold, matching the check constraint on `content.relations`.
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

pub const DEFAULT_LIST_LIMIT: i64 = 50;

pub struct ListRelationsQuery {
    pub source_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub relation_type: Option<String>,
    /// Restricts the listing to one state.
    /// `None` lists every state, so a caller that does not pass `status` sees deprecated and archived relations along with every other state.
    pub status: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl Default for ListRelationsQuery {
    fn default() -> Self {
        Self {
            source_id: None,
            target_id: None,
            relation_type: None,
            status: None,
            limit: DEFAULT_LIST_LIMIT,
            offset: 0,
        }
    }
}

pub const DEFAULT_NEIGHBORS_LIMIT: i64 = 20;

/// A relation together with the entity on the other end of it, relative to the entity `neighbors` was called for.
/// `direction` is `"out"` when the queried entity is the relation's source (the neighbor is the target) and `"in"` when it's the target (the neighbor is the source).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Neighbor {
    pub relation_id: Uuid,
    pub relation_type: String,
    pub direction: String,
    #[schema(value_type = Object)]
    pub properties: Value,
    pub entity: EntityRecord,
}

#[derive(Iden)]
enum Relations {
    Table,
    Id,
    WorkspaceId,
    SourceId,
    TargetId,
    RelationType,
    Properties,
    Status,
    CreatedAt,
}

fn relation_columns() -> [Relations; 8] {
    [
        Relations::Id,
        Relations::WorkspaceId,
        Relations::SourceId,
        Relations::TargetId,
        Relations::RelationType,
        Relations::Properties,
        Relations::Status,
        Relations::CreatedAt,
    ]
}

/// Validates that relation_type doesn't conflict with the source/target entity_types.
/// The metaschema definition is resolved against the schema the source entity was actually created with (the row `entities.schema_id` points to), as with `entities::update`, so existing relationships between entities don't silently break even as the active schema evolves.
async fn validate_relation_type<C>(
    conn: &mut C,
    workspace_id: Uuid,
    source: &entities::EntityRecord,
    target: &entities::EntityRecord,
    relation_type: &str,
) -> Result<(), YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    // `schemas::get_by_id`'s bound; see `entities::update`'s where clause for why this still has to be restated.
    schemas::SchemaRow: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let schema = schemas::get_by_id(conn, workspace_id, source.schema_id).await?;

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

/// Creates a new relation: verifies both the source and target entities exist and that relation_type matches the metaschema's source/target constraint, then persists it.
// `schemas::SchemaRow` is `pub(crate)`, not fully `pub` (see its definition for why).
#[allow(private_bounds)]
pub async fn create<C>(
    conn: &mut C,
    workspace_id: Uuid,
    input: CreateRelationInput,
) -> Result<RelationRecord, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    RelationRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
    EntityRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
    // `schemas::get_by_id`'s bound, needed by `validate_relation_type`; see `entities::update`'s where clause for why this still has to be restated.
    schemas::SchemaRow: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let source = entities::get(conn, workspace_id, input.source_id).await?;
    let target = entities::get(conn, workspace_id, input.target_id).await?;
    validate_relation_type(conn, workspace_id, &source, &target, &input.relation_type).await?;

    let properties = if input.properties.is_null() {
        json!({})
    } else {
        input.properties
    };

    let (sql, values) = Query::insert()
        .into_table(C::schema_table("content", Relations::Table))
        .columns([
            Relations::WorkspaceId,
            Relations::SourceId,
            Relations::TargetId,
            Relations::RelationType,
            Relations::Properties,
        ])
        .values_panic([
            workspace_id.into(),
            input.source_id.into(),
            input.target_id.into(),
            input.relation_type.clone().into(),
            properties.into(),
        ])
        .returning(Query::returning().columns(relation_columns()))
        .build_sqlx(C::builder());

    sqlx::query_as_with::<_, RelationRecord, _>(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .map_err(|err| match err.as_database_error() {
            Some(db_err) if db_err.is_unique_violation() => YorishiroError::Conflict {
                message: format!(
                    "relation '{}' between '{}' and '{}' already exists",
                    input.relation_type, input.source_id, input.target_id
                ),
            },
            // There's a TOCTOU window between checking source/target existence and the INSERT, during which another transaction could delete the entity.
            // An FK violation is that race surfacing, so it's treated as NotFound just like the upfront check.
            Some(db_err) if db_err.is_foreign_key_violation() => {
                YorishiroError::not_found(format!(
                    "source '{}' or target '{}' no longer exists",
                    input.source_id, input.target_id
                ))
            }
            _ => YorishiroError::Internal(err.into()),
        })
}

pub async fn get<C>(
    conn: &mut C,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<RelationRecord, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    RelationRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let (sql, values) = Query::select()
        .columns(relation_columns())
        .from(C::schema_table("content", Relations::Table))
        .and_where(Expr::col(Relations::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Relations::Id).eq(id))
        .build_sqlx(C::builder());

    sqlx::query_as_with::<_, RelationRecord, _>(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("relation '{id}' was not found")))
}

/// Moves a relation to another state.
/// Retiring a relation this way keeps the record that it existed, which deleting it does not; traversal stops following it either way.
pub async fn set_status<C>(
    conn: &mut C,
    workspace_id: Uuid,
    id: Uuid,
    status: &str,
) -> Result<RelationRecord, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    RelationRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    if !is_valid_relation_status(status) {
        return Err(YorishiroError::ValidationFailed {
            message: format!("'{status}' is not a relation status"),
            details: vec![crate::error::ValidationDetail {
                field: "/status".to_string(),
                problem: format!("expected one of {}", RELATION_STATUSES.join(", ")),
            }],
            hint: format!(
                "use one of {}: traversal follows '{RELATION_STATUS_ACTIVE}' only",
                RELATION_STATUSES.join(", ")
            ),
        });
    }

    let (sql, values) = Query::update()
        .table(C::schema_table("content", Relations::Table))
        .values([(Relations::Status, status.into())])
        .and_where(Expr::col(Relations::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Relations::Id).eq(id))
        .returning(Query::returning().columns(relation_columns()))
        .build_sqlx(C::builder());

    sqlx::query_as_with::<_, RelationRecord, _>(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("relation '{id}' was not found")))
}

pub async fn delete<C>(conn: &mut C, workspace_id: Uuid, id: Uuid) -> Result<(), YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
{
    let (sql, values) = Query::delete()
        .from_table(C::schema_table("content", Relations::Table))
        .and_where(Expr::col(Relations::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Relations::Id).eq(id))
        .build_sqlx(C::builder());

    let result = sqlx::query_with(&sql, values)
        .execute(&mut *conn)
        .await
        .internal()?;

    if C::rows_affected(result) == 0 {
        Err(YorishiroError::not_found(format!(
            "relation '{id}' was not found"
        )))
    } else {
        Ok(())
    }
}

pub async fn list<C>(
    conn: &mut C,
    workspace_id: Uuid,
    query: ListRelationsQuery,
) -> Result<Vec<RelationRecord>, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    RelationRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let limit = query.limit.clamp(1, 200);
    let offset = query.offset.max(0);

    let mut builder = Query::select();
    builder
        .columns(relation_columns())
        .from(C::schema_table("content", Relations::Table))
        .and_where(Expr::col(Relations::WorkspaceId).eq(workspace_id));
    if let Some(source_id) = query.source_id {
        builder.and_where(Expr::col(Relations::SourceId).eq(source_id));
    }
    if let Some(target_id) = query.target_id {
        builder.and_where(Expr::col(Relations::TargetId).eq(target_id));
    }
    if let Some(relation_type) = query.relation_type {
        builder.and_where(Expr::col(Relations::RelationType).eq(relation_type));
    }
    if let Some(status) = query.status {
        builder.and_where(Expr::col(Relations::Status).eq(status));
    }
    builder
        .order_by(Relations::CreatedAt, Order::Desc)
        .limit(limit as u64)
        .offset(offset as u64);
    let (sql, values) = builder.build_sqlx(C::builder());

    sqlx::query_as_with::<_, RelationRecord, _>(&sql, values)
        .fetch_all(&mut *conn)
        .await
        .internal()
}

/// Fetches every relation for the tenant, with no pagination limit, for a full-tenant export.
pub async fn export_all<C>(
    conn: &mut C,
    workspace_id: Uuid,
) -> Result<Vec<RelationRecord>, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    RelationRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let (sql, values) = Query::select()
        .columns(relation_columns())
        .from(C::schema_table("content", Relations::Table))
        .and_where(Expr::col(Relations::WorkspaceId).eq(workspace_id))
        .order_by(Relations::CreatedAt, Order::Asc)
        .build_sqlx(C::builder());

    sqlx::query_as_with::<_, RelationRecord, _>(&sql, values)
        .fetch_all(&mut *conn)
        .await
        .internal()
}

/// Counts how many relations a workspace holds, for workspace-detail summaries.
pub async fn count<C>(conn: &mut C, workspace_id: Uuid) -> Result<i64, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    (i64,): for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let (sql, values) = Query::select()
        .expr(Func::count(Expr::col(Asterisk)))
        .from(C::schema_table("content", Relations::Table))
        .and_where(Expr::col(Relations::WorkspaceId).eq(workspace_id))
        .build_sqlx(C::builder());
    let (count,): (i64,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .internal()?;
    Ok(count)
}

#[derive(sqlx::FromRow)]
struct NeighborRow {
    relation_id: Uuid,
    relation_type: String,
    direction: String,
    properties: Value,
    // Only used to drive the SQL-level ORDER BY; not read on the Rust side.
    #[allow(dead_code)]
    relation_created_at: DateTime<Utc>,
    entity_id: Uuid,
    entity_tenant_id: Uuid,
    entity_schema_id: Uuid,
    entity_schema_version: i32,
    entity_type: String,
    entity_data: Value,
    entity_created_at: DateTime<Utc>,
    entity_updated_at: DateTime<Utc>,
    entity_created_by: Option<Uuid>,
    entity_updated_by: Option<Uuid>,
}

impl NeighborRow {
    fn into_neighbor(self) -> Neighbor {
        Neighbor {
            relation_id: self.relation_id,
            relation_type: self.relation_type,
            direction: self.direction,
            properties: self.properties,
            entity: entities::EntityRecord {
                id: self.entity_id,
                workspace_id: self.entity_tenant_id,
                schema_id: self.entity_schema_id,
                schema_version: self.entity_schema_version,
                entity_type: self.entity_type,
                data: self.entity_data,
                created_at: self.entity_created_at,
                updated_at: self.entity_updated_at,
                created_by: self.entity_created_by,
                updated_by: self.entity_updated_by,
            },
        }
    }
}

/// Same shape as [`NeighborRow`] plus the pivot id that produced it, for [`neighbors_batch`]'s `CROSS JOIN LATERAL` result where multiple pivots' rows come back in a single result set.
#[derive(sqlx::FromRow)]
struct BatchNeighborRow {
    pivot_id: Uuid,
    relation_id: Uuid,
    relation_type: String,
    direction: String,
    properties: Value,
    // Only used to drive the SQL-level ORDER BY; not read on the Rust side (same convention as `NeighborRow::relation_created_at`).
    #[allow(dead_code)]
    relation_created_at: DateTime<Utc>,
    entity_id: Uuid,
    entity_tenant_id: Uuid,
    entity_schema_id: Uuid,
    entity_schema_version: i32,
    entity_type: String,
    entity_data: Value,
    entity_created_at: DateTime<Utc>,
    entity_updated_at: DateTime<Utc>,
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
            entity: entities::EntityRecord {
                id: self.entity_id,
                workspace_id: self.entity_tenant_id,
                schema_id: self.entity_schema_id,
                schema_version: self.entity_schema_version,
                entity_type: self.entity_type,
                data: self.entity_data,
                created_at: self.entity_created_at,
                updated_at: self.entity_updated_at,
                created_by: self.entity_created_by,
                updated_by: self.entity_updated_by,
            },
        }
    }
}

/// Returns the entities directly connected to `entity_id` by a relation, in either direction, together with the relation_type and direction of each connection.
/// Ordered by the relation's creation time, most recent first.
// `NeighborRow` stays private: its `FromRow` impl is generic over any `Row`, so no external caller ever needs to name it to satisfy this bound.
#[allow(private_bounds)]
pub async fn neighbors<C>(
    conn: &mut C,
    workspace_id: Uuid,
    entity_id: Uuid,
    limit: i64,
) -> Result<Vec<Neighbor>, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    NeighborRow: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
    Uuid: for<'q> sqlx::Encode<'q, C::Db> + sqlx::Type<C::Db>,
    i64: for<'q> sqlx::Encode<'q, C::Db> + sqlx::Type<C::Db>,
    for<'a> <C::Db as sqlx::Database>::Arguments<'a>: sqlx::IntoArguments<'a, C::Db>,
{
    let limit = limit.clamp(1, 200);

    // sea-query can express a UNION ALL of two joined SELECTs and an ORDER BY/LIMIT applied to the union, but only by building each branch as a full, separate `Query::select()` and combining them: for a query already this wide (14 aliased output columns each side, two joins, a computed direction literal), that ends up materially harder to read than the plain SQL below, with no behavioral upside.
    // Kept raw as a deliberate readability call, not because it's structurally inexpressible (contrast the `db.rs`/`auth.rs` session-command and SECURITY DEFINER cases, which have no builder form at all).
    let rows = sqlx::query_as::<C::Db, NeighborRow>(
        "SELECT r.id AS relation_id, r.relation_type, 'out' AS direction, r.properties, \
                r.created_at AS relation_created_at, \
                e.id AS entity_id, e.workspace_id AS entity_tenant_id, e.schema_id AS entity_schema_id, \
                e.schema_version AS entity_schema_version, e.entity_type, e.data AS entity_data, \
                e.created_at AS entity_created_at, e.updated_at AS entity_updated_at, \
                e.created_by AS entity_created_by, e.updated_by AS entity_updated_by \
         FROM content.relations r \
         JOIN content.entities e ON e.id = r.target_id AND e.workspace_id = r.workspace_id \
         WHERE r.workspace_id = $1 AND r.source_id = $2 AND r.status = 'active' \
         UNION ALL \
         SELECT r.id, r.relation_type, 'in' AS direction, r.properties, r.created_at, \
                e.id, e.workspace_id, e.schema_id, e.schema_version, e.entity_type, e.data, \
                e.created_at, e.updated_at, e.created_by, e.updated_by \
         FROM content.relations r \
         JOIN content.entities e ON e.id = r.source_id AND e.workspace_id = r.workspace_id \
         WHERE r.workspace_id = $1 AND r.target_id = $2 AND r.status = 'active' \
         ORDER BY relation_created_at DESC \
         LIMIT $3",
    )
    .bind(workspace_id)
    .bind(entity_id)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await
    .internal()?;

    Ok(rows.into_iter().map(NeighborRow::into_neighbor).collect())
}

/// Batched form of [`neighbors`]: looks up up to `limit` neighbors of every id in `pivot_ids` in one round trip instead of one `neighbors()` call per id, via `CROSS JOIN LATERAL` so each pivot still gets its own `limit`-bounded result (a plain `WHERE source_id = ANY(...) LIMIT n` would apply `limit` across the whole batch instead of per pivot, which is not the same query).
/// Same truncation convention as `neighbors`: pass `desired_limit + 1` in and compare the returned count against `desired_limit` to detect truncation.
/// Returns a map from pivot id to its neighbors; a pivot with no relations at all is absent from the map rather than present with an empty vec.
/// A duplicate id in `pivot_ids` contributes only once (deduped before querying): `unnest` would otherwise drive the lateral subquery twice for that id and double its entry in the result map.
pub async fn neighbors_batch(
    conn: &mut PgConnection,
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

    // For each pivot, the lateral subquery is the same UNION ALL/ORDER BY/LIMIT as `neighbors`, just correlated against `pivot.id` instead of a single bound parameter: `source_id` drives the 'out' branch and `target_id` drives the 'in' branch, exactly as in `neighbors`, so per-pivot direction semantics are unchanged.
    // The lateral's own ORDER BY/LIMIT already picks the right *set* of up-to-`limit` rows per pivot; the outer ORDER BY is what then guarantees those rows come back to Rust in per-pivot, most-recent-first order too:
    // `CROSS JOIN LATERAL` doesn't otherwise promise the driving/inner join order is preserved across pivots, and `recall_context` relies on that order when it truncates.
    let rows = sqlx::query_as::<_, BatchNeighborRow>(
        "SELECT pivot.id AS pivot_id, n.relation_id, n.relation_type, n.direction, \
                n.properties, n.relation_created_at, n.entity_id, n.entity_tenant_id, \
                n.entity_schema_id, n.entity_schema_version, n.entity_type, n.entity_data, \
                n.entity_created_at, n.entity_updated_at, n.entity_created_by, \
                n.entity_updated_by \
         FROM unnest($2::uuid[]) AS pivot(id) \
         CROSS JOIN LATERAL ( \
             SELECT r.id AS relation_id, r.relation_type, 'out' AS direction, r.properties, \
                    r.created_at AS relation_created_at, \
                    e.id AS entity_id, e.workspace_id AS entity_tenant_id, \
                    e.schema_id AS entity_schema_id, e.schema_version AS entity_schema_version, \
                    e.entity_type, e.data AS entity_data, e.created_at AS entity_created_at, \
                    e.updated_at AS entity_updated_at, e.created_by AS entity_created_by, \
                    e.updated_by AS entity_updated_by \
             FROM content.relations r \
             JOIN content.entities e ON e.id = r.target_id AND e.workspace_id = r.workspace_id \
             WHERE r.workspace_id = $1 AND r.source_id = pivot.id AND r.status = 'active' \
             UNION ALL \
             SELECT r.id, r.relation_type, 'in' AS direction, r.properties, r.created_at, \
                    e.id, e.workspace_id, e.schema_id, e.schema_version, e.entity_type, e.data, \
                    e.created_at, e.updated_at, e.created_by, e.updated_by \
             FROM content.relations r \
             JOIN content.entities e ON e.id = r.source_id AND e.workspace_id = r.workspace_id \
             WHERE r.workspace_id = $1 AND r.target_id = pivot.id AND r.status = 'active' \
             ORDER BY relation_created_at DESC \
             LIMIT $3 \
         ) AS n \
         ORDER BY pivot.id, n.relation_created_at DESC",
    )
    .bind(workspace_id)
    .bind(pivot_ids)
    .bind(limit)
    .fetch_all(&mut *conn)
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

#[cfg(test)]
#[path = "../../../tests/models/relations/mod.rs"]
mod tests;
