use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, FromQueryResult, QueryOrder, QuerySelect, SqlErr, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::db_enum::db_enum;
use crate::error::{ResultExt, ValidationDetail, ValidationErrorCode, YorishiroError};
pub use crate::models::_entities::entity_relations::{ActiveModel, Entity, Model};
use crate::models::entity_entities::{self, EntityRecord};

db_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum RelationStatus {
        Active = "active",
        Deprecated = "deprecated",
        Archived = "archived",
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RelationRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    pub properties: Value,
    pub status: RelationStatus,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
}

impl TryFrom<Model> for RelationRecord {
    type Error = YorishiroError;

    fn try_from(model: Model) -> Result<Self, Self::Error> {
        let status = RelationStatus::from_db_str(&model.status).ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!("unknown relation status: {}", model.status))
        })?;
        Ok(Self {
            id: model.id,
            workspace_id: model.workspace_id,
            source_id: model.source_id,
            target_id: model.target_id,
            relation_type: model.relation_type,
            properties: model.properties,
            status,
            created_at: model.created_at,
        })
    }
}

#[loco_rs::prelude::async_trait]
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

pub(crate) fn parse_relation_status(value: &str) -> Result<RelationStatus, YorishiroError> {
    RelationStatus::from_db_str(value).ok_or_else(|| YorishiroError::ValidationFailed {
        message: format!("'{value}' is not a relation status"),
        details: vec![ValidationDetail {
            field: "/status".into(),
            problem: "expected active, deprecated, or archived".into(),
            code: ValidationErrorCode::Other,
            expected: Some("active, deprecated, archived".into()),
            actual: Some(value.into()),
        }],
        hint: "use active, deprecated, or archived".into(),
    })
}

pub struct CreateRelationInput {
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    pub properties: Value,
}

pub(crate) struct SetRelationStatusInput {
    pub id: Uuid,
    pub status: RelationStatus,
}

#[derive(Default)]
pub struct ListRelationsQuery {
    pub source_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub relation_type: Option<String>,
    /// Restricts the listing to one state.
    /// `None` lists every state, so a caller that does not pass `status` sees deprecated and archived relations along with every other state.
    pub status: Option<RelationStatus>,
    pub page: crate::models::pagination::ListParams,
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
        crate::models::schema_schemas::get_by_id(conn, workspace_id, source.schema_id).await?;

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
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn create(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    input: CreateRelationInput,
) -> Result<RelationRecord, YorishiroError> {
    let source = entity_entities::get(conn, workspace_id, input.source_id).await?;
    let target = entity_entities::get(conn, workspace_id, input.target_id).await?;
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
    active
        .insert(conn)
        .await
        .map_err(|err| {
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
        .and_then(TryInto::try_into)
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub(crate) async fn get(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<RelationRecord, YorishiroError> {
    use crate::models::_entities::entity_relations::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(id))
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("relation '{id}' was not found")))
        .and_then(TryInto::try_into)
}

/// Moves a relation to another state.
/// Retiring a relation this way keeps the record that it existed, which deleting it does not; traversal stops following it either way.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub(crate) async fn set_status(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    input: SetRelationStatusInput,
) -> Result<RelationRecord, YorishiroError> {
    let existing = get(conn, workspace_id, input.id).await?;

    let active = ActiveModel {
        id: ActiveValue::Unchanged(existing.id),
        status: ActiveValue::Set(input.status.as_db_str().to_string()),
        ..Default::default()
    };
    // A concurrent delete between `get` above and this update surfaces as `DbErr::RecordNotUpdated`, not a row.
    // Map it to the same 404 the upfront `get` would have returned had it lost the race instead, rather than letting `.internal()` turn it into a 500.
    active
        .update(conn)
        .await
        .map_err(|err| match err {
            DbErr::RecordNotUpdated => {
                YorishiroError::not_found(format!("relation '{}' was not found", input.id))
            }
            err => YorishiroError::Internal(err.into()),
        })
        .and_then(TryInto::try_into)
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn delete(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::entity_relations::Column;

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
pub(crate) async fn list(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    query: ListRelationsQuery,
) -> Result<Vec<RelationRecord>, YorishiroError> {
    use crate::models::_entities::entity_relations::Column;

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
        select = select.filter(Column::Status.eq(status.as_db_str()));
    }

    select
        .order_by_desc(Column::CreatedAt)
        .limit(query.page.limit() as u64)
        .offset(query.page.offset() as u64)
        .all(conn)
        .await
        .internal()
        .and_then(|rows| rows.into_iter().map(TryInto::try_into).collect())
}

/// Counts how many relations a workspace holds, for workspace-detail summaries.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn count(conn: &impl ConnectionTrait, workspace_id: Uuid) -> Result<i64, YorishiroError> {
    use crate::models::_entities::entity_relations::Column;

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
pub(crate) async fn export_all(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<RelationRecord>, YorishiroError> {
    use crate::models::_entities::entity_relations::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(Column::CreatedAt)
        .all(conn)
        .await
        .internal()
        .and_then(|rows| rows.into_iter().map(TryInto::try_into).collect())
}

pub(crate) const MIN_NEIGHBORS_LIMIT: i64 = 1;
pub(crate) const MAX_NEIGHBORS_LIMIT: i64 = 200;
pub(crate) const DEFAULT_NEIGHBORS_LIMIT: i64 = 20;

/// A relation together with the entity on the other end of it, relative to the entity `neighbors_batch` was called for.
/// `direction` is `"out"` when the queried entity is the relation's source (the neighbor is the target) and `"in"` when it's the target (the neighbor is the source).
#[derive(Clone, Serialize)]
pub(crate) struct Neighbor {
    pub relation_id: Uuid,
    pub relation_type: String,
    pub direction: String,
    pub properties: Value,
    pub entity: EntityRecord,
}

#[derive(FromQueryResult)]
struct BatchNeighborRow {
    pivot_id: Uuid,
    relation_id: Uuid,
    relation_type: String,
    direction: String,
    properties: Value,
    #[sea_orm(nested(prefix = "entity_"))]
    entity: EntityRecord,
}

impl BatchNeighborRow {
    fn into_neighbor(self) -> Neighbor {
        Neighbor {
            relation_id: self.relation_id,
            relation_type: self.relation_type,
            direction: self.direction,
            properties: self.properties,
            entity: self.entity,
        }
    }
}

/// Batched neighbor lookup: finds up to `limit` neighbors of every id in `pivot_ids` in one round trip instead of one call per id, via `CROSS JOIN LATERAL` so each pivot still gets its own `limit`-bounded result (a plain `WHERE source_id = ANY(...) LIMIT n` would apply `limit` across the whole batch instead of per pivot, which is not the same query).
///
/// Returns a map from pivot id to its neighbors; a pivot with no relations at all is absent from the map rather than present with an empty vec.
/// A duplicate id in `pivot_ids` contributes only once (deduped before querying).
/// SeaQuery preserves the per-pivot bound with PostgreSQL LATERAL/unnest and SQLite row_number() partitions.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub(crate) async fn neighbors_batch(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    pivot_ids: &[Uuid],
    limit: i64,
) -> Result<std::collections::HashMap<Uuid, Vec<Neighbor>>, YorishiroError> {
    let limit = limit.clamp(MIN_NEIGHBORS_LIMIT, MAX_NEIGHBORS_LIMIT);
    let pivot_ids: Vec<Uuid> = pivot_ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    if pivot_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let rows = neighbor_rows(conn, workspace_id, &pivot_ids, limit).await?;

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

/// Fetches one hop in one query, retaining the per-pivot bound on both backends.
async fn neighbor_rows(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    pivot_ids: &[Uuid],
    limit: i64,
) -> Result<Vec<BatchNeighborRow>, YorishiroError> {
    use crate::models::_entities::{entity_entities as entity, entity_relations as relation};
    use sea_orm::sea_query::{
        Alias, Expr, ExprTrait, Func, JoinType, Order, Query, UnionType, WindowStatement,
    };
    let sqlite = conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite;
    let branch = |pivot, neighbor, direction: &str| {
        let mut select = Query::select();
        select
            .expr_as(Expr::col((relation::Entity, pivot)), "pivot_id")
            .expr_as(
                Expr::col((relation::Entity, relation::Column::Id)),
                "relation_id",
            )
            .column((relation::Entity, relation::Column::RelationType))
            .expr_as(Expr::val(direction), "direction")
            .column((relation::Entity, relation::Column::Properties))
            .expr_as(
                Expr::col((relation::Entity, relation::Column::CreatedAt)),
                "relation_created_at",
            );
        for (column, alias) in [
            (entity::Column::Id, "entity_id"),
            (entity::Column::WorkspaceId, "entity_workspace_id"),
            (entity::Column::SchemaId, "entity_schema_id"),
            (entity::Column::SchemaVersion, "entity_schema_version"),
            (entity::Column::EntityType, "entity_entity_type"),
            (entity::Column::Data, "entity_data"),
            (entity::Column::CreatedAt, "entity_created_at"),
            (entity::Column::UpdatedAt, "entity_updated_at"),
            (entity::Column::CreatedBy, "entity_created_by"),
            (entity::Column::UpdatedBy, "entity_updated_by"),
        ] {
            select.expr_as(Expr::col((entity::Entity, column)), alias);
        }
        select
            .from(relation::Entity)
            .join(
                JoinType::InnerJoin,
                entity::Entity,
                Expr::col((entity::Entity, entity::Column::Id))
                    .equals((relation::Entity, neighbor))
                    .and(
                        Expr::col((entity::Entity, entity::Column::WorkspaceId))
                            .equals((relation::Entity, relation::Column::WorkspaceId)),
                    ),
            )
            .and_where(
                Expr::col((relation::Entity, relation::Column::WorkspaceId)).eq(if sqlite {
                    sea_orm::Value::from(workspace_id.as_bytes().to_vec())
                } else {
                    workspace_id.into()
                }),
            );
        if sqlite {
            select.and_where(
                Expr::col((relation::Entity, pivot))
                    .is_in(pivot_ids.iter().map(|id| id.as_bytes().to_vec())),
            );
        } else {
            select.and_where(Expr::col((relation::Entity, pivot)).equals(Alias::new("pivot")));
        }
        select.and_where(
            Expr::col((relation::Entity, relation::Column::Status))
                .eq(RelationStatus::Active.as_db_str()),
        );
        select
    };
    let mut neighbors = branch(
        relation::Column::SourceId,
        relation::Column::TargetId,
        "out",
    );
    neighbors.union(
        UnionType::All,
        branch(relation::Column::TargetId, relation::Column::SourceId, "in"),
    );
    if !sqlite {
        neighbors
            .order_by(Alias::new("relation_created_at"), Order::Desc)
            .limit(limit as u64);
        let query = Query::select()
            .column((Alias::new("n"), sea_orm::sea_query::Asterisk))
            .from_function(
                Func::cust(Alias::new("unnest")).arg(Expr::val(pivot_ids.to_vec())),
                Alias::new("pivot"),
            )
            .join_lateral(
                JoinType::InnerJoin,
                neighbors,
                Alias::new("n"),
                Expr::val(true),
            )
            .order_by((Alias::new("n"), Alias::new("pivot_id")), Order::Asc)
            .order_by(
                (Alias::new("n"), Alias::new("relation_created_at")),
                Order::Desc,
            )
            .to_owned();
        return BatchNeighborRow::find_by_statement(conn.get_database_backend().build(&query))
            .all(conn)
            .await
            .internal();
    }
    let ranked = Query::select()
        .column((Alias::new("n"), sea_orm::sea_query::Asterisk))
        .expr_window_as(
            Func::cust(Alias::new("row_number")),
            WindowStatement::partition_by(Alias::new("pivot_id"))
                .order_by(Alias::new("relation_created_at"), Order::Desc)
                .to_owned(),
            "position",
        )
        .from_subquery(neighbors, Alias::new("n"))
        .to_owned();
    let query = Query::select()
        .column(sea_orm::sea_query::Asterisk)
        .from_subquery(ranked, Alias::new("ranked"))
        .and_where(Expr::col(Alias::new("position")).lte(limit))
        .order_by(Alias::new("pivot_id"), Order::Asc)
        .order_by(Alias::new("relation_created_at"), Order::Desc)
        .to_owned();
    // A frontier can exceed SQLite's bind limit; SeaQuery safely renders typed UUID literals.
    BatchNeighborRow::find_by_statement(Statement::from_string(
        conn.get_database_backend(),
        query.to_string(sea_orm::sea_query::SqliteQueryBuilder),
    ))
    .all(conn)
    .await
    .internal()
}
