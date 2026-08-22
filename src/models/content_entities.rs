use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveValue, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub use super::_entities::content_entities::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, ValidationDetail, YorishiroError};
use crate::metaschema;

pub type ContentEntities = Entity;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    /// Stamps `updated_at` on every update whose caller didn't already set it explicitly.
    /// Checks `!is_set()` rather than `is_unchanged()`: an `ActiveModel` built with
    /// `..Default::default()` (this crate's usual pattern, as opposed to loading a `Model` and
    /// calling `.into_active_model()`) leaves untouched fields `NotSet`, not `Unchanged`, and
    /// `is_unchanged()` only matches the latter.
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if !insert && !self.updated_at.is_set() {
            let mut this = self;
            this.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
            Ok(this)
        } else {
            Ok(self)
        }
    }
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}

/// The RLS-scoped request path's view of a `content_entities` row.
/// A distinct type from the generated `Model` because `Model` carries `embedding`
/// (`Option<PgVector>`), which this API never returns: the search/embedding pipeline manages
/// that column separately, and serializing a raw vector out of a CRUD response was never the
/// old API's shape either.
/// `created_by`/`updated_by` are `None` for entities touched by an unattributed (service/automation) API key, since there's no user to record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub schema_id: Uuid,
    pub schema_version: i32,
    pub entity_type: String,
    pub data: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub updated_by: Option<Uuid>,
}

impl From<Model> for EntityRecord {
    fn from(row: Model) -> Self {
        EntityRecord {
            id: row.id,
            workspace_id: row.workspace_id,
            schema_id: row.schema_id,
            schema_version: row.schema_version,
            entity_type: row.entity_type,
            data: row.data,
            created_at: row.created_at.into(),
            updated_at: row.updated_at.into(),
            created_by: row.created_by,
            updated_by: row.updated_by,
        }
    }
}

pub struct CreateEntityInput {
    pub schema_name: String,
    pub entity_type: String,
    pub data: Value,
}

pub const DEFAULT_LIST_LIMIT: i64 = 50;

pub struct ListEntitiesQuery {
    pub entity_type: Option<String>,
    /// JSONB containment filter (`data @> filter`), e.g. `{"status": "active"}`.
    pub filter: Option<Value>,
    /// Restricts results to entities created against this schema version.
    /// Entities keep the version they were written against, so this selects the entities a given version produced rather than the ones that would validate against it today.
    pub schema_version: Option<i32>,
    pub limit: i64,
    pub offset: i64,
}

impl Default for ListEntitiesQuery {
    fn default() -> Self {
        Self {
            entity_type: None,
            filter: None,
            schema_version: None,
            limit: DEFAULT_LIST_LIMIT,
            offset: 0,
        }
    }
}

/// Escapes `~`/`/` per RFC 6901 before embedding a value as a JSON Pointer segment.
fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// Represents where a validation error occurred as a JSON Pointer.
/// For `required` violations, `instance_path()` alone only points at the containing object and doesn't say which property is missing, so the missing property name is appended.
fn error_field_pointer(err: &jsonschema::ValidationError<'_>) -> String {
    let base = err.instance_path().to_string();
    if let jsonschema::error::ValidationErrorKind::Required { property } = err.kind()
        && let Some(name) = property.as_str()
    {
        format!("{base}/{}", escape_pointer_segment(name))
    } else {
        base
    }
}

/// Validates `data` against the JSON Schema generated from the entity_type definition.
/// Reuses `entity_type_to_json_schema`'s schema as-is so validation logic isn't duplicated between entities and the MCP inputSchema.
pub fn validate_data(
    entity_type_def: &metaschema::EntityTypeDef,
    data: &Value,
) -> Result<(), YorishiroError> {
    let schema = metaschema::entity_type_to_json_schema(entity_type_def);
    let validator = jsonschema::validator_for(&schema)
        .map_err(|err| YorishiroError::Internal(anyhow::anyhow!(err.to_string())))?;

    let details: Vec<ValidationDetail> = validator
        .iter_errors(data)
        .map(|err| ValidationDetail {
            field: error_field_pointer(&err),
            problem: err.to_string(),
        })
        .collect();

    if details.is_empty() {
        Ok(())
    } else {
        Err(YorishiroError::ValidationFailed {
            message: "entity data does not conform to its schema".into(),
            details,
            hint: "Check the entity_type field definitions against the submitted data".into(),
        })
    }
}

fn resolve_entity_type<'a>(
    definition: &'a metaschema::MetaSchemaDefinition,
    entity_type: &str,
) -> Result<&'a metaschema::EntityTypeDef, YorishiroError> {
    definition.entity_types.get(entity_type).ok_or_else(|| {
        YorishiroError::not_found(format!(
            "entity_type '{entity_type}' is not defined in schema '{}'",
            definition.name
        ))
    })
}

/// Checks the workspace's `max_entities` cap (billing/quota enforcement) before an insert.
/// `NULL` means unlimited, which is the default so self-hosted deployments are never capped unless an operator explicitly sets a limit.
/// The app role only has SELECT on `identity_workspaces`, which is enough to read this column without needing write access to the control-plane table.
async fn check_entity_quota(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    let max_entities = crate::models::identity_workspaces::Entity::find_by_id(workspace_id)
        .select_only()
        .column(crate::models::_entities::identity_workspaces::Column::MaxEntities)
        .into_tuple::<Option<i32>>()
        .one(conn)
        .await
        .internal()?
        .flatten();

    let Some(max) = max_entities else {
        return Ok(());
    };

    let count = count(conn, workspace_id).await?;

    if count >= i64::from(max) {
        Err(YorishiroError::Conflict {
            message: format!(
                "workspace '{workspace_id}' has reached its entity limit ({max}); \
                 raise max_entities or delete existing entities"
            ),
        })
    } else {
        Ok(())
    }
}

/// Counts how many entities a workspace holds, for both quota enforcement (`create`, above) and workspace-detail summaries.
pub async fn count(conn: &impl ConnectionTrait, workspace_id: Uuid) -> Result<i64, YorishiroError> {
    use super::_entities::content_entities::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .count(conn)
        .await
        .internal()
        .map(|n| n as i64)
}

/// Creates a new entity: resolves the schema name to its currently active schema, checks that the entity_type exists in that version, validates `data`, and persists the result.
/// `created_by` is the acting user's ID (from `AuthContext::user_id`), or `None` for an unattributed service/automation API key.
///
/// The quota check and insert are serialized with a workspace-scoped advisory lock: without it, concurrent creates could each read a count under `max_entities` and both insert, overshooting the cap.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`. That
/// transaction is also this function's lock/quota/insert scope: it does not open a nested
/// transaction of its own, since the request transaction already is the unit of work (the
/// caller commits it after this returns, via `Authorized::commit()`).
pub async fn create(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    input: CreateEntityInput,
    created_by: Option<Uuid>,
) -> Result<EntityRecord, YorishiroError> {
    crate::db::lock_for_update(conn, &workspace_id.to_string())
        .await
        .internal()?;

    check_entity_quota(conn, workspace_id).await?;

    // Before resolving the schema, so an empty workspace is told it is empty.
    // Resolving first would report the schema name as not found, which reads as a typo rather than as "nothing has been defined here yet".
    if crate::models::identity_workspaces::is_schema_pending(conn, workspace_id).await? {
        return Err(YorishiroError::ValidationFailed {
            message: format!(
                "workspace '{workspace_id}' has no schema yet, so there is nothing to \
                 validate this entity against"
            ),
            details: vec![],
            hint: "create a schema first: POST /api/schemas, or the create_schema tool. \
                   list_templates shows the built-in ones."
                .to_string(),
        });
    }

    let schema =
        crate::models::content_schemas::get_active_schema(conn, workspace_id, &input.schema_name)
            .await?;
    let entity_type_def = resolve_entity_type(&schema.definition, &input.entity_type)?;
    validate_data(entity_type_def, &input.data)?;

    let active = ActiveModel {
        workspace_id: ActiveValue::Set(workspace_id),
        schema_id: ActiveValue::Set(schema.id),
        schema_version: ActiveValue::Set(schema.version),
        entity_type: ActiveValue::Set(input.entity_type),
        data: ActiveValue::Set(input.data),
        created_by: ActiveValue::Set(created_by),
        ..Default::default()
    };
    let row = active.insert(conn).await.internal()?;

    Ok(row.into())
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn get(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<EntityRecord, YorishiroError> {
    use super::_entities::content_entities::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(id))
        .one(conn)
        .await
        .internal()?
        .map(EntityRecord::from)
        .ok_or_else(|| YorishiroError::not_found(format!("entity '{id}' was not found")))
}

/// Fully replaces an existing entity's `data`.
/// Validation is done against the schema version the entity was actually created with (i.e. the row's `schema_id`), so existing entities don't silently break compatibility even if the active version has since moved on.
/// `updated_by` is the acting user's ID, or `None` for an unattributed service/automation API key: this overwrites whatever `updated_by` the previous update (if any) left behind.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn update(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
    data: Value,
    updated_by: Option<Uuid>,
) -> Result<EntityRecord, YorishiroError> {
    let existing = get(conn, workspace_id, id).await?;
    let schema =
        crate::models::content_schemas::get_by_id(conn, workspace_id, existing.schema_id).await?;
    let entity_type_def = resolve_entity_type(&schema.definition, &existing.entity_type)?;
    validate_data(entity_type_def, &data)?;

    let active = ActiveModel {
        id: ActiveValue::Unchanged(id),
        data: ActiveValue::Set(data),
        updated_by: ActiveValue::Set(updated_by),
        ..Default::default()
    };
    let row = active.update(conn).await.internal()?;

    Ok(row.into())
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn delete(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<(), YorishiroError> {
    use super::_entities::content_entities::Column;

    let result = Entity::delete_many()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(id))
        .exec(conn)
        .await
        .internal()?;

    if result.rows_affected == 0 {
        Err(YorishiroError::not_found(format!(
            "entity '{id}' was not found"
        )))
    } else {
        Ok(())
    }
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
///
/// `query.filter` (JSONB containment, `data @> filter`) is the one condition here the entity
/// API's `ColumnTrait` can't express (`ColumnTrait::contains` builds a `LIKE '%...%'`, unrelated
/// to Postgres's `@>` operator): it's added as a raw `Expr::cust_with_values` condition mixed
/// into the same `.filter()` chain as the ordinary column filters, per this repository's rule
/// that raw SQL stays scoped to what the entity layer genuinely can't express.
pub async fn list(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    query: ListEntitiesQuery,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    use super::_entities::content_entities::Column;

    let limit = query.limit.clamp(1, 200);
    let offset = query.offset.max(0);

    let mut select = Entity::find().filter(Column::WorkspaceId.eq(workspace_id));
    if let Some(entity_type) = query.entity_type {
        select = select.filter(Column::EntityType.eq(entity_type));
    }
    if let Some(filter) = query.filter {
        select = select.filter(Expr::cust_with_values("data @> $1", [filter]));
    }
    if let Some(schema_version) = query.schema_version {
        select = select.filter(Column::SchemaVersion.eq(schema_version));
    }

    select
        .order_by_desc(Column::CreatedAt)
        .limit(limit as u64)
        .offset(offset as u64)
        .all(conn)
        .await
        .internal()
        .map(|rows| rows.into_iter().map(EntityRecord::from).collect())
}
