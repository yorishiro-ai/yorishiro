use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, QueryOrder, QuerySelect, SqlErr};
use serde_json::{Value, json};
use uuid::Uuid;

pub use super::_entities::content_relations::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, ValidationDetail, YorishiroError};
use crate::models::content_entities::{self, EntityRecord};

pub type ContentRelations = Entity;

/// The generated `Model` already matches this API's response shape (unlike `content_entities`,
/// there's no `embedding`-style column to exclude), so this is an alias rather than a distinct
/// struct.
pub type RelationRecord = Model;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        Ok(self)
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
/// Callers validate before writing so an unknown value is a 422 naming the field, not a
/// constraint violation surfacing as a 500.
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
    /// `None` lists every state, so a caller that does not pass `status` sees deprecated and
    /// archived relations along with every other state.
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

/// Validates that `relation_type` doesn't conflict with the source/target entity_types.
/// The metaschema definition is resolved against the schema the source entity was actually
/// created with (the row's `schema_id`), as with `content_entities::update`, so existing
/// relationships between entities don't silently break even as the active schema evolves.
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

/// Creates a new relation: verifies both the source and target entities exist and that
/// `relation_type` matches the metaschema's source/target constraint, then persists it.
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
            // A TOCTOU window between checking source/target existence and the INSERT, during
            // which another transaction could delete the entity. Treated as NotFound, same as
            // the upfront check.
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
/// Retiring a relation this way keeps the record that it existed, which deleting it does not;
/// traversal stops following it either way.
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
    active.update(conn).await.internal()
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

    let limit = query.limit.clamp(1, 200);
    let offset = query.offset.max(0);

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
        .limit(limit as u64)
        .offset(offset as u64)
        .all(conn)
        .await
        .internal()
}
