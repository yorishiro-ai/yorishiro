use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveValue, QueryOrder, QuerySelect, Statement};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub use super::_entities::content_entities::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, ValidationDetail, YorishiroError};
use crate::metaschema;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::sqlite_generated_id(db, this.id);
        this.updated_at = crate::db::stamped_updated_at(insert, this.updated_at);
        Ok(this)
    }
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}

/// The RLS-scoped request path's view of a `content_entities` row.
/// Distinct from the generated `Model` because it excludes `embedding` (stored separately in `content_entity_embeddings`), so SQLite deserialization does not trip on the PgVector type.
/// `created_by`/`updated_by` are `None` for entities touched by an unattributed API key.
#[derive(Clone, Debug, Serialize, Deserialize, sea_orm::FromQueryResult)]
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

/// SQLite variant of `EntityRecord` that accepts UUIDs as hex strings.
/// SQLite stores UUIDs as TEXT (36 chars) rather than BLOB (16 bytes),
/// and SeaORM's `FromQueryResult` expects BLOB for `Uuid` columns.
#[derive(sea_orm::FromQueryResult)]
pub struct EntityRecordStr {
    pub id: String,
    pub workspace_id: String,
    pub schema_id: String,
    pub schema_version: i32,
    pub entity_type: String,
    pub data: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
}

impl EntityRecordStr {
    /// Converts the hex-string UUIDs into `EntityRecord` with parsed `Uuid` values.
    pub fn into_record(self) -> EntityRecord {
        EntityRecord {
            id: uuid::Uuid::parse_str(&self.id).expect("id is a valid UUID"),
            workspace_id: uuid::Uuid::parse_str(&self.workspace_id)
                .expect("workspace_id is a valid UUID"),
            schema_id: uuid::Uuid::parse_str(&self.schema_id).expect("schema_id is a valid UUID"),
            schema_version: self.schema_version,
            entity_type: self.entity_type,
            data: self.data,
            created_at: self.created_at,
            updated_at: self.updated_at,
            created_by: self.created_by.map(|s| {
                uuid::Uuid::parse_str(&s).expect("created_by is a valid UUID")
            }),
            updated_by: self.updated_by.map(|s| {
                uuid::Uuid::parse_str(&s).expect("updated_by is a valid UUID")
            }),
        }
    }
}

pub struct CreateEntityInput {
    pub schema_name: String,
    pub entity_type: String,
    pub data: Value,
}

#[derive(Default)]
pub struct ListEntitiesQuery {
    pub entity_type: Option<String>,
    /// JSONB containment filter (`data @> filter`), e.g. `{"status": "active"}`.
    pub filter: Option<Value>,
    /// Restricts results to entities created against this schema version.
    /// Entities keep the version they were written against, so this selects the entities a given version produced, not the ones that would validate against it today.
    pub schema_version: Option<i32>,
    pub page: super::pagination::ListParams,
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

/// Checks the workspace's `max_entities` cap before an insert.
/// `NULL` means unlimited, the default for the enterprise edition.
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

/// Counts how many entities a workspace holds, for quota enforcement and workspace-detail summaries.
pub async fn count(conn: &impl ConnectionTrait, workspace_id: Uuid) -> Result<i64, YorishiroError> {
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return count_sqlite(conn, workspace_id).await;
    }
    use super::_entities::content_entities::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .count(conn)
        .await
        .internal()
        .map(|n| n as i64)
}

/// SQLite variant of `count`: uses raw SQL with hex-string UUIDs.
async fn count_sqlite(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<i64, YorishiroError> {
    use sea_orm::{DatabaseBackend, Statement};

    let sql = format!(
        "SELECT count(*) FROM content_entities WHERE workspace_id = '{}'",
        workspace_id
    );
    let stmt = Statement::from_string(DatabaseBackend::Sqlite, sql);
    let rows = conn.query_all_raw(stmt).await.internal()?;
    let row = rows
        .first()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("no result from count")))?;
    let count: i64 = row.try_get("", "count(*)").internal()?;
    Ok(count)
}

/// Creates a new entity: resolves the schema name to its currently active schema, checks that the entity_type exists in that version, validates `data`, and persists the result.
/// `created_by` is the acting user's ID, or `None` for an unattributed service/automation API key.
///
/// The quota check and insert are serialized with a workspace-scoped advisory lock: without it, concurrent creates could each read a count under `max_entities` and both insert, overshooting the cap.
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

    // Before resolving the schema, so an empty workspace is told it is empty rather than reporting the schema name as not found.
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

    // On SQLite, UUID columns are stored as hex strings in TEXT columns.
    // SeaORM serializes `Uuid` as 16-byte binary, which FK checks cannot
    // match against the hex-string FK targets. Convert to hex for the
    // insert so the FK constraints evaluate correctly.
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        let id = match active.id {
            sea_orm::ActiveValue::NotSet => Uuid::now_v7(),
            sea_orm::ActiveValue::Set(id) | sea_orm::ActiveValue::Unchanged(id) => id,
        };
        let mut active = active;
        active.id = ActiveValue::Set(id);
        return create_sqlite(conn, active).await.internal();
    }

    active.insert(conn).await.internal().map(EntityRecord::from)
}

/// SQLite variant of `create`: inserts the row directly with hex-string UUIDs so FK
/// constraints (which compare against hex-string TEXT columns) evaluate correctly.
async fn create_sqlite(
    conn: &impl ConnectionTrait,
    active: ActiveModel,
) -> Result<EntityRecord, DbErr> {
    use sea_orm::{DatabaseBackend, Statement};

    let entity_id = match active.id {
        sea_orm::ActiveValue::Set(id) | sea_orm::ActiveValue::Unchanged(id) => id,
        _ => return Err(DbErr::Custom("id is not set".into())),
    };
    let workspace_id = match active.workspace_id {
        sea_orm::ActiveValue::Set(id) | sea_orm::ActiveValue::Unchanged(id) => id,
        _ => return Err(DbErr::Custom("workspace_id is not set".into())),
    };
    let schema_id = match active.schema_id {
        sea_orm::ActiveValue::Set(id) | sea_orm::ActiveValue::Unchanged(id) => id,
        _ => return Err(DbErr::Custom("schema_id is not set".into())),
    };
    let schema_version: i32 = match active.schema_version {
        sea_orm::ActiveValue::Set(v) | sea_orm::ActiveValue::Unchanged(v) => v,
        _ => return Err(DbErr::Custom("schema_version is not set".into())),
    };
    let entity_type: String = match active.entity_type {
        sea_orm::ActiveValue::Set(v) | sea_orm::ActiveValue::Unchanged(v) => v,
        _ => return Err(DbErr::Custom("entity_type is not set".into())),
    };
    let data: serde_json::Value = match active.data {
        sea_orm::ActiveValue::Set(v) | sea_orm::ActiveValue::Unchanged(v) => v,
        _ => return Err(DbErr::Custom("data is not set".into())),
    };
    let created_by = match active.created_by {
        sea_orm::ActiveValue::Set(Some(id)) | sea_orm::ActiveValue::Unchanged(Some(id)) => Some(id),
        _ => None,
    };
    let updated_by = match active.updated_by {
        sea_orm::ActiveValue::Set(Some(id)) | sea_orm::ActiveValue::Unchanged(Some(id)) => Some(id),
        _ => None,
    };
    let now = chrono::Utc::now().to_rfc3339();

    // Build INSERT with hex-string UUIDs (TEXT columns)
    let insert_sql = format!(
        "INSERT INTO content_entities (id, workspace_id, schema_id, schema_version, entity_type, data, created_by, updated_by, created_at, updated_at) \
         VALUES ('{}', '{}', '{}', {}, '{}', '{}', {}, {}, '{}', '{}')",
        entity_id,
        workspace_id,
        schema_id,
        schema_version,
        entity_type,
        data.to_string().replace('\'', "''"),
        created_by
            .map(|u| format!("'{}'", u))
            .unwrap_or("NULL".to_string()),
        updated_by
            .map(|u| format!("'{}'", u))
            .unwrap_or("NULL".to_string()),
        now,
        now
    );

    conn.execute_raw(Statement::from_string(DatabaseBackend::Sqlite, insert_sql))
        .await?;

    // Fetch the inserted row using raw SQL to avoid SeaORM's binary UUID decoding issue
    let select_sql = format!(
        "SELECT id, workspace_id, schema_id, schema_version, entity_type, data, created_by, updated_by, created_at, updated_at FROM content_entities WHERE id = '{}'",
        entity_id
    );
    let rows = conn
        .query_all_raw(Statement::from_string(DatabaseBackend::Sqlite, select_sql))
        .await?;
    let row = rows.first().ok_or(DbErr::RecordNotFound(
        "entity not found after insert".to_string(),
    ))?;

    // Decode UUIDs manually since try_get::<Uuid> expects 16-byte binary
    let id = Uuid::parse_str(&row.try_get::<String>("", "id")?)
        .map_err(|e| DbErr::Custom(e.to_string()))?;
    let workspace_id_out = Uuid::parse_str(&row.try_get::<String>("", "workspace_id")?)
        .map_err(|e| DbErr::Custom(e.to_string()))?;
    let schema_id_out = Uuid::parse_str(&row.try_get::<String>("", "schema_id")?)
        .map_err(|e| DbErr::Custom(e.to_string()))?;
    let entity_type_out: String = row.try_get("", "entity_type")?;
    let schema_version_out: i32 = row.try_get("", "schema_version")?;
    let data_raw: serde_json::Value = row.try_get("", "data")?;
    let created_by_out: Option<Uuid> = row
        .try_get::<Option<String>>("", "created_by")
        .ok()
        .flatten()
        .map(|s| Uuid::parse_str(&s))
        .transpose()
        .map_err(|e: uuid::Error| DbErr::Custom(e.to_string()))?;
    let updated_by_out: Option<Uuid> = row
        .try_get::<Option<String>>("", "updated_by")
        .ok()
        .flatten()
        .map(|s| Uuid::parse_str(&s))
        .transpose()
        .map_err(|e: uuid::Error| DbErr::Custom(e.to_string()))?;
    let created_at: chrono::DateTime<chrono::Utc> = row.try_get("", "created_at")?;
    let updated_at: chrono::DateTime<chrono::Utc> = row
        .try_get("", "updated_at")
        .ok()
        .unwrap_or_else(chrono::Utc::now);

    Ok(EntityRecord {
        id,
        workspace_id: workspace_id_out,
        schema_id: schema_id_out,
        schema_version: schema_version_out,
        entity_type: entity_type_out,
        data: data_raw,
        created_by: created_by_out,
        updated_by: updated_by_out,
        created_at,
        updated_at,
    })
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn get(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<EntityRecord, YorishiroError> {
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return get_sqlite(conn, workspace_id, id).await;
    }
    use super::_entities::content_entities::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(id))
        .into_model::<EntityRecord>()
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("entity '{id}' was not found")))
}

/// SQLite variant of `get`: uses raw SQL with hex-string UUIDs in the WHERE clause.
async fn get_sqlite(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<EntityRecord, YorishiroError> {
    use sea_orm::{DatabaseBackend, Statement};

    let sql = format!(
        "SELECT id, workspace_id, schema_id, schema_version, entity_type, data, created_by, updated_by, created_at, updated_at FROM content_entities WHERE id = '{}' AND workspace_id = '{}'",
        id, workspace_id
    );
    let stmt = Statement::from_string(DatabaseBackend::Sqlite, sql);
    let rows = conn.query_all_raw(stmt).await.internal()?;
    let row = rows
        .first()
        .ok_or_else(|| YorishiroError::not_found(format!("entity '{id}' was not found")))?;

    let id_out = Uuid::parse_str(&row.try_get::<String>("", "id").internal()?).internal()?;
    let workspace_id_out =
        Uuid::parse_str(&row.try_get::<String>("", "workspace_id").internal()?).internal()?;
    let schema_id =
        Uuid::parse_str(&row.try_get::<String>("", "schema_id").internal()?).internal()?;
    let schema_version: i32 = row.try_get("", "schema_version").internal()?;
    let entity_type: String = row.try_get("", "entity_type").internal()?;
    let data_raw: serde_json::Value = row.try_get("", "data").internal()?;
    let created_by = row
        .try_get::<Option<String>>("", "created_by")
        .ok()
        .flatten()
        .map(|s| Uuid::parse_str(&s))
        .transpose()
        .ok()
        .flatten();
    let updated_by = row
        .try_get::<Option<String>>("", "updated_by")
        .ok()
        .flatten()
        .map(|s| Uuid::parse_str(&s))
        .transpose()
        .ok()
        .flatten();
    let created_at: DateTime<Utc> = row.try_get("", "created_at").internal()?;
    let updated_at: DateTime<Utc> = row.try_get("", "updated_at").internal()?;

    Ok(EntityRecord {
        id: id_out,
        workspace_id: workspace_id_out,
        schema_id,
        schema_version,
        entity_type,
        data: data_raw,
        created_by,
        updated_by,
        created_at,
        updated_at,
    })
}

/// [`get`], batched: one query for every id instead of one query per id.
/// An id with no matching row (deleted, or belonging to another workspace) is simply absent from
/// the returned map, mirroring `content_relations::neighbors_batch`'s own no-match-is-no-entry
/// convention rather than erroring.
pub async fn get_batch(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, EntityRecord>, YorishiroError> {
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return get_batch_sqlite(conn, workspace_id, ids).await;
    }
    use super::_entities::content_entities::Column;

    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let rows = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.is_in(ids.iter().copied()))
        .into_model::<EntityRecord>()
        .all(conn)
        .await
        .internal()?;

    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}

/// SQLite variant of `get_batch`: uses raw SQL with hex-string UUIDs.
async fn get_batch_sqlite(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, EntityRecord>, YorishiroError> {
    use sea_orm::{DatabaseBackend, Statement};

    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let hex_ids: Vec<String> = ids.iter().map(|u| u.to_string()).collect();
    let placeholders = hex_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT id, workspace_id, schema_id, schema_version, entity_type, data, created_by, updated_by, created_at, updated_at \
         FROM content_entities WHERE workspace_id = '{}' AND id IN ({})",
        workspace_id, placeholders
    );

    let values: Vec<sea_orm::Value> = hex_ids
        .iter()
        .map(|s| sea_orm::Value::String(Some(s.clone())))
        .collect();
    let stmt = Statement::from_sql_and_values(DatabaseBackend::Sqlite, &sql, values);
    let rows = conn.query_all_raw(stmt).await.internal()?;

    let mut map = std::collections::HashMap::new();
    for row in rows {
        let id_out = Uuid::parse_str(&row.try_get::<String>("", "id").internal()?).internal()?;
        let ws =
            Uuid::parse_str(&row.try_get::<String>("", "workspace_id").internal()?).internal()?;
        let schema_id =
            Uuid::parse_str(&row.try_get::<String>("", "schema_id").internal()?).internal()?;
        let schema_version: i32 = row.try_get("", "schema_version").internal()?;
        let entity_type: String = row.try_get("", "entity_type").internal()?;
        let data_raw: serde_json::Value = row.try_get("", "data").internal()?;
        let created_by = row
            .try_get::<Option<String>>("", "created_by")
            .ok()
            .flatten()
            .map(|s| Uuid::parse_str(&s))
            .transpose()
            .ok()
            .flatten();
        let updated_by = row
            .try_get::<Option<String>>("", "updated_by")
            .ok()
            .flatten()
            .map(|s| Uuid::parse_str(&s))
            .transpose()
            .ok()
            .flatten();
        let created_at: DateTime<Utc> = row.try_get("", "created_at").internal()?;
        let updated_at: DateTime<Utc> = row.try_get("", "updated_at").internal()?;

        map.insert(
            id_out,
            EntityRecord {
                id: id_out,
                workspace_id: ws,
                schema_id,
                schema_version,
                entity_type,
                data: data_raw,
                created_by,
                updated_by,
                created_at,
                updated_at,
            },
        );
    }

    Ok(map)
}

/// Fully replaces an existing entity's `data`.
/// Validation is done against the schema version the entity was actually created with (the row's `schema_id`), so existing entities don't silently break compatibility even if the active version has since moved on.
/// `updated_by` is the acting user's ID, or `None` for an unattributed service/automation API key.
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

    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return update_sqlite(conn, workspace_id, id, data, updated_by)
            .await
            .internal();
    }

    let active = ActiveModel {
        id: ActiveValue::Unchanged(id),
        data: ActiveValue::Set(data),
        updated_by: ActiveValue::Set(updated_by),
        ..Default::default()
    };
    active.update(conn).await.internal().map(EntityRecord::from)
}

/// SQLite variant of `update`: uses raw SQL with hex-string UUIDs so FK constraints
/// (which compare against hex-string TEXT columns) evaluate correctly.
async fn update_sqlite(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
    data: serde_json::Value,
    updated_by: Option<Uuid>,
) -> Result<EntityRecord, DbErr> {
    use sea_orm::{DatabaseBackend, Statement};

    let now = chrono::Utc::now().to_rfc3339();

    // Build UPDATE with hex-string UUIDs in WHERE clause
    let update_sql = format!(
        "UPDATE content_entities SET data = '{}', updated_by = {}, updated_at = '{}' \
         WHERE id = '{}' AND workspace_id = '{}'",
        data.to_string().replace('\'', "''"),
        updated_by
            .map(|u| format!("'{}'", u))
            .unwrap_or("NULL".to_string()),
        now,
        id,
        workspace_id
    );

    let result = conn
        .execute_raw(Statement::from_string(DatabaseBackend::Sqlite, update_sql))
        .await?;

    if result.rows_affected() == 0 {
        return Err(DbErr::Custom("None of the records are updated".into()));
    }

    // Fetch the updated row
    let select_sql = format!(
        "SELECT id, workspace_id, schema_id, schema_version, entity_type, data, created_by, updated_by, created_at, updated_at FROM content_entities WHERE id = '{}'",
        id
    );
    let rows = conn
        .query_all_raw(Statement::from_string(DatabaseBackend::Sqlite, select_sql))
        .await?;
    let row = rows.first().ok_or(DbErr::RecordNotFound(
        "entity not found after update".to_string(),
    ))?;

    let id_out = Uuid::parse_str(&row.try_get::<String>("", "id")?)
        .map_err(|e| DbErr::Custom(e.to_string()))?;
    let workspace_id_out = Uuid::parse_str(&row.try_get::<String>("", "workspace_id")?)
        .map_err(|e| DbErr::Custom(e.to_string()))?;
    let schema_id = Uuid::parse_str(&row.try_get::<String>("", "schema_id")?)
        .map_err(|e| DbErr::Custom(e.to_string()))?;
    let schema_version: i32 = row.try_get("", "schema_version")?;
    let entity_type: String = row.try_get("", "entity_type")?;
    let data_raw: serde_json::Value = row.try_get("", "data")?;
    let created_by: Option<Uuid> = row
        .try_get::<Option<String>>("", "created_by")
        .ok()
        .flatten()
        .map(|s| Uuid::parse_str(&s))
        .transpose()
        .map_err(|e: uuid::Error| DbErr::Custom(e.to_string()))?;
    let updated_by_out: Option<Uuid> = row
        .try_get::<Option<String>>("", "updated_by")
        .ok()
        .flatten()
        .map(|s| Uuid::parse_str(&s))
        .transpose()
        .map_err(|e: uuid::Error| DbErr::Custom(e.to_string()))?;
    let created_at: chrono::DateTime<chrono::Utc> = row.try_get("", "created_at")?;
    let updated_at: chrono::DateTime<chrono::Utc> = row.try_get("", "updated_at")?;

    Ok(EntityRecord {
        id: id_out,
        workspace_id: workspace_id_out,
        schema_id,
        schema_version,
        entity_type,
        data: data_raw,
        created_by,
        updated_by: updated_by_out,
        created_at,
        updated_at,
    })
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn delete(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<(), YorishiroError> {
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return delete_sqlite(conn, workspace_id, id).await.internal();
    }

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

/// SQLite variant of `delete`: uses raw SQL with hex-string UUIDs so FK constraints
/// (which compare against hex-string TEXT columns) evaluate correctly.
async fn delete_sqlite(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<(), DbErr> {
    use sea_orm::{DatabaseBackend, Statement};

    let sql = format!(
        "DELETE FROM content_entities WHERE id = '{}' AND workspace_id = '{}'",
        id, workspace_id
    );
    let result = conn
        .execute_raw(Statement::from_string(DatabaseBackend::Sqlite, sql))
        .await?;

    if result.rows_affected() == 0 {
        Err(DbErr::Custom("None of the records are updated".into()))
    } else {
        Ok(())
    }
}

/// `query.filter` (JSONB containment, `data @> filter`) is the one condition here `ColumnTrait` can't express (`ColumnTrait::contains` builds a `LIKE '%...%'`, unrelated to Postgres's `@>` operator), so it's added as a raw `Expr::cust_with_values` condition instead.
pub async fn list(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    query: ListEntitiesQuery,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return list_sqlite(conn, workspace_id, query).await;
    }
    use super::_entities::content_entities::Column;

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

    let select = select
        .order_by_desc(Column::CreatedAt)
        .limit(query.page.limit() as u64)
        .offset(query.page.offset() as u64);

    select
        .into_model::<EntityRecord>()
        .all(conn)
        .await
        .internal()
}

/// SQLite variant of `list`: uses raw SQL with hex-string UUIDs in the WHERE clause.
async fn list_sqlite(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    query: ListEntitiesQuery,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    use sea_orm::{DatabaseBackend, Statement};

    // Build SQL with hex-string UUIDs — all values are inline, no placeholders.
    let mut conditions = vec![format!("workspace_id = '{}'", workspace_id)];

    if let Some(entity_type) = query.entity_type {
        conditions.push(format!(
            "entity_type = '{}'",
            entity_type.replace('\'', "''")
        ));
    }
    if let Some(filter) = query.filter {
        // On SQLite, JSON is stored as TEXT; use LIKE on the serialized data column.
        conditions.push(format!(
            "data LIKE '%{}%'",
            serde_json::to_string(&filter)
                .internal()?
                .replace('\'', "''")
        ));
    }
    if let Some(schema_version) = query.schema_version {
        conditions.push(format!("schema_version = {}", schema_version));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT id, workspace_id, schema_id, schema_version, entity_type, data, created_by, updated_by, created_at, updated_at FROM content_entities{} \
         ORDER BY created_at DESC LIMIT {} OFFSET {}",
        where_clause,
        query.page.limit(),
        query.page.offset()
    );

    let stmt = Statement::from_string(DatabaseBackend::Sqlite, sql);
    let rows = conn.query_all_raw(stmt).await.internal()?;

    let mut result = Vec::new();
    for row in rows {
        let id_out = Uuid::parse_str(&row.try_get::<String>("", "id").internal()?).internal()?;
        let ws =
            Uuid::parse_str(&row.try_get::<String>("", "workspace_id").internal()?).internal()?;
        let schema_id =
            Uuid::parse_str(&row.try_get::<String>("", "schema_id").internal()?).internal()?;
        let schema_version: i32 = row.try_get("", "schema_version").internal()?;
        let entity_type: String = row.try_get("", "entity_type").internal()?;
        let data_raw: serde_json::Value = row.try_get("", "data").internal()?;
        let created_by = row
            .try_get::<Option<String>>("", "created_by")
            .ok()
            .flatten()
            .map(|s| Uuid::parse_str(&s))
            .transpose()
            .ok()
            .flatten();
        let updated_by = row
            .try_get::<Option<String>>("", "updated_by")
            .ok()
            .flatten()
            .map(|s| Uuid::parse_str(&s))
            .transpose()
            .ok()
            .flatten();
        let created_at: DateTime<Utc> = row.try_get("", "created_at").internal()?;
        let updated_at: DateTime<Utc> = row.try_get("", "updated_at").internal()?;

        result.push(EntityRecord {
            id: id_out,
            workspace_id: ws,
            schema_id,
            schema_version,
            entity_type,
            data: data_raw,
            created_by,
            updated_by,
            created_at,
            updated_at,
        });
    }

    Ok(result)
}

/// Fetches every entity for the workspace, with no pagination limit, for a full-workspace data export.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn export_all(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return export_all_sqlite(conn, workspace_id).await;
    }
    use super::_entities::content_entities::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(Column::CreatedAt)
        .into_model::<EntityRecord>()
        .all(conn)
        .await
        .internal()
}

/// SQLite variant of `export_all`: uses raw SQL with hex-string UUIDs.
async fn export_all_sqlite(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    use sea_orm::{DatabaseBackend, Statement};

    let sql = format!(
        "SELECT id, workspace_id, schema_id, schema_version, entity_type, data, created_by, updated_by, created_at, updated_at FROM content_entities WHERE workspace_id = '{}' ORDER BY created_at ASC",
        workspace_id
    );
    let stmt = Statement::from_string(DatabaseBackend::Sqlite, sql);
    let rows = conn.query_all_raw(stmt).await.internal()?;

    let mut result = Vec::new();
    for row in rows {
        let id_out = Uuid::parse_str(&row.try_get::<String>("", "id").internal()?).internal()?;
        let ws =
            Uuid::parse_str(&row.try_get::<String>("", "workspace_id").internal()?).internal()?;
        let schema_id =
            Uuid::parse_str(&row.try_get::<String>("", "schema_id").internal()?).internal()?;
        let schema_version: i32 = row.try_get("", "schema_version").internal()?;
        let entity_type: String = row.try_get("", "entity_type").internal()?;
        let data_raw: serde_json::Value = row.try_get("", "data").internal()?;
        let created_by = row
            .try_get::<Option<String>>("", "created_by")
            .ok()
            .flatten()
            .map(|s| Uuid::parse_str(&s))
            .transpose()
            .ok()
            .flatten();
        let updated_by = row
            .try_get::<Option<String>>("", "updated_by")
            .ok()
            .flatten()
            .map(|s| Uuid::parse_str(&s))
            .transpose()
            .ok()
            .flatten();
        let created_at: DateTime<Utc> = row.try_get("", "created_at").internal()?;
        let updated_at: DateTime<Utc> = row.try_get("", "updated_at").internal()?;

        result.push(EntityRecord {
            id: id_out,
            workspace_id: ws,
            schema_id,
            schema_version,
            entity_type,
            data: data_raw,
            created_by,
            updated_by,
            created_at,
            updated_at,
        });
    }

    Ok(result)
}

/// How one entity stands relative to the active version of its schema.
///
/// Entities are migrated lazily: a schema gaining a version does not rewrite rows written against earlier ones.
/// This exists so a reader can tell whether a field is absent because nobody filled it in or because it did not exist when the entity was written.
#[derive(Clone, Debug, Serialize)]
pub struct EntityDrift {
    pub entity_id: Uuid,
    pub entity_type: String,
    /// The version this entity was written against.
    pub schema_version: i32,
    /// The newest active version of the same schema.
    pub active_schema_version: i32,
    /// Fields the active version defines that this entity's version did not.
    /// Empty when the entity is current, and empty as well when the newer version only changed fields the entity already carries.
    pub missing_fields: Vec<DriftField>,
}

/// A field an entity predates.
#[derive(Clone, Debug, Serialize)]
pub struct DriftField {
    pub name: String,
    /// The field's type in the active version, so a caller can tell what would go there.
    pub r#type: metaschema::FieldTypeName,
    /// Whether the active version marks it required.
    /// A required field an old entity lacks is the case worth surfacing: the entity is valid under its own version and would not be under the current one.
    pub required: bool,
}

/// Reports how `entity_id` stands against the active version of its schema.
pub async fn drift(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
) -> Result<EntityDrift, YorishiroError> {
    let entity = get(conn, workspace_id, entity_id).await?;
    let own = super::content_schemas::get_by_id(conn, workspace_id, entity.schema_id).await?;
    let active = super::content_schemas::get_active_schema(conn, workspace_id, &own.name).await?;

    // The entity's own type definition may be absent from the active version: the type was dropped.
    // Nothing is "missing" in that case; the whole type is, which the version numbers already say.
    let own_fields = own
        .definition
        .entity_types
        .get(&entity.entity_type)
        .map(|def| &def.fields);
    let active_fields = active
        .definition
        .entity_types
        .get(&entity.entity_type)
        .map(|def| &def.fields);

    let missing_fields = match (own_fields, active_fields) {
        (Some(own_fields), Some(active_fields)) => active_fields
            .iter()
            .filter(|(name, _)| !own_fields.contains_key(*name))
            .map(|(name, def)| DriftField {
                name: name.clone(),
                r#type: def.r#type,
                required: def.required,
            })
            .collect(),
        _ => Vec::new(),
    };

    Ok(EntityDrift {
        entity_id: entity.id,
        entity_type: entity.entity_type,
        schema_version: entity.schema_version,
        active_schema_version: active.version,
        missing_fields,
    })
}

/// What a batch migration would find, without doing it.
/// Counts entities before anything is touched, since a workspace accumulates entities spread across schema versions.
#[derive(Clone, Debug, Serialize)]
pub struct MigrationDryRun {
    pub schema_name: String,
    /// The version everything would be brought to.
    pub active_version: i32,
    pub total_entities: i64,
    /// Already on the active version.
    /// Nothing to do for these.
    pub current: i64,
    /// On an older version, but missing no field the active version requires: they validate as they stand and only their version marker is behind.
    pub behind_but_valid: i64,
    /// On an older version and missing at least one field the active version requires.
    /// These are what a batch migration has to fill in.
    pub needs_values: i64,
    /// Per entity type, so an operator can see whether the work is spread or concentrated.
    pub by_entity_type: Vec<DryRunByType>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DryRunByType {
    pub entity_type: String,
    pub behind: i64,
    pub needs_values: i64,
    /// The required fields those entities lack, so the report names the work rather than only counting it.
    pub missing_required: Vec<String>,
}

/// Counts what a batch migration to `schema_name`'s active version would face.
/// Reads only.
///
/// The counting is done in one query per (entity_type, schema_id) group rather than one per entity: a workspace can hold far more entities than it holds distinct old versions, and the answer is the same either way.
pub async fn migration_dry_run(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_name: &str,
) -> Result<MigrationDryRun, YorishiroError> {
    use super::_entities::content_entities::Column;

    let active = super::content_schemas::get_active_schema(conn, workspace_id, schema_name).await?;

    // (entity_type, schema_id, count) for everything under this schema name, whatever version.
    #[derive(sea_orm::FromQueryResult)]
    struct GroupedCount {
        entity_type: String,
        schema_id: Uuid,
        count: i64,
    }

    let rows: Vec<GroupedCount> = Entity::find()
        .select_only()
        .column(Column::EntityType)
        .column(Column::SchemaId)
        .column_as(Column::Id.count(), "count")
        .filter(Column::WorkspaceId.eq(workspace_id))
        .join(
            sea_orm::JoinType::InnerJoin,
            super::_entities::content_entities::Relation::ContentSchemas.def(),
        )
        .filter(super::_entities::content_schemas::Column::Name.eq(schema_name))
        .group_by(Column::EntityType)
        .group_by(Column::SchemaId)
        .into_model::<GroupedCount>()
        .all(conn)
        .await
        .internal()?;

    let mut total = 0i64;
    let mut current = 0i64;
    let mut behind_valid = 0i64;
    let mut needs_values = 0i64;
    let mut by_type: std::collections::BTreeMap<String, DryRunByType> =
        std::collections::BTreeMap::new();

    // Each distinct old version is fetched once, not once per entity.
    let mut definitions: std::collections::HashMap<Uuid, metaschema::MetaSchemaDefinition> =
        std::collections::HashMap::new();

    for row in rows {
        total += row.count;

        if row.schema_id == active.id {
            current += row.count;
            continue;
        }

        let old = match definitions.get(&row.schema_id) {
            Some(def) => def.clone(),
            None => {
                let record =
                    super::content_schemas::get_by_id(conn, workspace_id, row.schema_id).await?;
                definitions.insert(row.schema_id, record.definition.clone());
                record.definition
            }
        };

        // Required in the active version, absent from the version these were written with.
        let missing: Vec<String> = match (
            active.definition.entity_types.get(&row.entity_type),
            old.entity_types.get(&row.entity_type),
        ) {
            (Some(active_type), Some(old_type)) => active_type
                .fields
                .iter()
                .filter(|(name, def)| def.required && !old_type.fields.contains_key(*name))
                .map(|(name, _)| name.clone())
                .collect(),
            _ => Vec::new(),
        };

        let entry = by_type
            .entry(row.entity_type.clone())
            .or_insert_with(|| DryRunByType {
                entity_type: row.entity_type,
                behind: 0,
                needs_values: 0,
                missing_required: Vec::new(),
            });
        entry.behind += row.count;

        if missing.is_empty() {
            behind_valid += row.count;
        } else {
            needs_values += row.count;
            entry.needs_values += row.count;
            for name in missing {
                if !entry.missing_required.contains(&name) {
                    entry.missing_required.push(name);
                }
            }
        }
    }

    Ok(MigrationDryRun {
        schema_name: schema_name.to_string(),
        active_version: active.version,
        total_entities: total,
        current,
        behind_but_valid: behind_valid,
        needs_values,
        by_entity_type: by_type.into_values().collect(),
    })
}

/// An entity's data as it stood before something overwrote it.
#[derive(Clone, Serialize, sea_orm::FromQueryResult)]
pub struct EntitySnapshot {
    pub id: Uuid,
    /// Groups the snapshots taken by one operation, so a batch is undone as a batch.
    pub job_id: Uuid,
    pub entity_id: Uuid,
    pub schema_id: Uuid,
    pub schema_version: i32,
    pub data: Value,
    pub created_at: DateTime<Utc>,
}

/// What undoing a job put back.
#[derive(Clone, Serialize)]
pub struct UndoReport {
    pub job_id: Uuid,
    /// Entities restored to the data they held before.
    pub restored: i64,
    /// Snapshots whose entity no longer exists.
    /// Counted rather than treated as an error: a batch partially undone leaves a workspace in a state nobody chose, and an entity deleted since is not a reason to refuse the rest.
    pub missing: i64,
}

/// Records what `entity_id` holds now, tagged with `job_id`.
///
/// One statement (`INSERT ... SELECT`), not a read followed by a write: under READ COMMITTED, two statements can see different committed data, so a separate read could snapshot an image the row no longer holds by the time the insert runs.
///
/// `content_entity_snapshots`'s RLS policy matches nothing rather than raising when no workspace is named, so a wrong `workspace_id` or missing entity silently inserts zero rows: `rows_affected == 0` is the only signal that catches it.
pub async fn snapshot(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
    job_id: Uuid,
) -> Result<(), YorishiroError> {
    let result = conn
        .execute_raw(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "INSERT INTO content_entity_snapshots \
                (job_id, workspace_id, entity_id, schema_id, schema_version, data) \
             SELECT $1, workspace_id, id, schema_id, schema_version, data \
               FROM content_entities \
              WHERE workspace_id = $2 AND id = $3",
            [job_id.into(), workspace_id.into(), entity_id.into()],
        ))
        .await
        .internal()?;

    if result.rows_affected() == 0 {
        return Err(YorishiroError::not_found(format!(
            "entity '{entity_id}' was not found"
        )));
    }
    Ok(())
}

/// Removes one entity's snapshot from `job_id`'s group.
///
/// For a caller that takes a snapshot before a write it isn't certain will land (`ee/`'s `infer_fill`, writing a model's guess straight to the entity): if that write then fails for a reason specific to it, the snapshot no longer describes a real change, and leaving it would let a later, unrelated edit to the same entity be misattributed to this job on undo.
pub async fn delete_snapshot(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
    job_id: Uuid,
) -> Result<(), YorishiroError> {
    use super::_entities::content_entity_snapshots;
    content_entity_snapshots::Entity::delete_many()
        .filter(content_entity_snapshots::Column::WorkspaceId.eq(workspace_id))
        .filter(content_entity_snapshots::Column::EntityId.eq(entity_id))
        .filter(content_entity_snapshots::Column::JobId.eq(job_id))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// Puts every entity in `job_id` back to what it held before.
///
/// An entity deleted since the snapshot is counted rather than failed: refusing the whole undo because one row is gone would leave the rest wrong.
///
/// Restores `schema_id` and `schema_version` alongside `data`, not just `data`: otherwise the entity would claim a version its restored data no longer matches.
/// Builds the `ActiveModel` directly rather than calling `update()`, which cannot set those two columns and would also re-validate and re-stamp `updated_by` for a restore that isn't a user edit.
pub async fn undo_job(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<UndoReport, YorishiroError> {
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return undo_job_sqlite(conn, workspace_id, job_id).await;
    }

    use super::_entities::content_entity_snapshots;

    let snapshots: Vec<EntitySnapshot> = content_entity_snapshots::Entity::find()
        .filter(content_entity_snapshots::Column::WorkspaceId.eq(workspace_id))
        .filter(content_entity_snapshots::Column::JobId.eq(job_id))
        // `id` as a tiebreaker: `created_at` alone is ambiguous when two snapshots land in the same tick.
        // Both are uuidv7 / time-ordered, so this is a second sort key.
        .order_by_asc(content_entity_snapshots::Column::CreatedAt)
        .order_by_asc(content_entity_snapshots::Column::Id)
        .into_model::<EntitySnapshot>()
        .all(conn)
        .await
        .internal()?;

    if snapshots.is_empty() {
        return Err(YorishiroError::not_found(format!(
            "no snapshots for job '{job_id}'"
        )));
    }

    let mut restored = 0i64;
    let mut missing = 0i64;

    for snap in &snapshots {
        // `snap.entity_id` is already workspace-scoped by the query above, so no extra filter is needed on the write.
        let active = ActiveModel {
            id: ActiveValue::Unchanged(snap.entity_id),
            data: ActiveValue::Set(snap.data.clone()),
            schema_id: ActiveValue::Set(snap.schema_id),
            schema_version: ActiveValue::Set(snap.schema_version),
            ..Default::default()
        };
        // `update_without_returning` rather than `active.update`: it still calls
        // `before_save` (unlike the raw `Entity::update(...)` builder) and raises
        // `DbErr::RecordNotUpdated` on no match, but decodes no `Model` on return
        // — the `Ok`/`RecordNotUpdated` outcome is all the caller needs.
        let result = active.update_without_returning(conn).await.map(|_| ());
        match result {
            Ok(()) => restored += 1,
            Err(DbErr::RecordNotUpdated) => missing += 1,
            Err(err) => return Err(err).internal(),
        }
    }

    Ok(UndoReport {
        job_id,
        restored,
        missing,
    })
}

/// SQLite variant of `undo_job`: queries snapshots with hex-string UUIDs so SeaORM's
/// `EntitySnapshot` model can decode them (UUID columns stored as hex in TEXT on SQLite).
async fn undo_job_sqlite(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<UndoReport, YorishiroError> {
    use sea_orm::{DatabaseBackend, Statement};

    // Fetch snapshots using hex-string UUIDs in WHERE clause
    let sql = format!(
        "SELECT id, job_id, workspace_id, entity_id, schema_id, schema_version, data, created_at \
         FROM content_entity_snapshots \
         WHERE workspace_id = '{}' AND job_id = '{}' \
         ORDER BY created_at ASC, id ASC",
        workspace_id, job_id
    );
    let stmt = Statement::from_string(DatabaseBackend::Sqlite, sql);
    let rows = conn.query_all_raw(stmt).await.internal()?;

    if rows.is_empty() {
        return Err(YorishiroError::not_found(format!(
            "no snapshots for job '{job_id}'"
        )));
    }

    let mut restored = 0i64;
    let mut missing = 0i64;
    let now = chrono::Utc::now().to_rfc3339();

    for row in &rows {
        let entity_id =
            Uuid::parse_str(&row.try_get::<String>("", "entity_id").internal()?).internal()?;
        let schema_id =
            Uuid::parse_str(&row.try_get::<String>("", "schema_id").internal()?).internal()?;
        let schema_version: i32 = row.try_get("", "schema_version").internal()?;
        let data: serde_json::Value = row.try_get("", "data").internal()?;

        // Build UPDATE with hex-string UUIDs in WHERE clause
        let update_sql = format!(
            "UPDATE content_entities SET data = '{}', schema_id = '{}', schema_version = {}, updated_at = '{}' \
             WHERE id = '{}' AND workspace_id = '{}'",
            data.to_string().replace('\'', "''"),
            schema_id,
            schema_version,
            now,
            entity_id,
            workspace_id
        );

        let result = conn
            .execute_raw(Statement::from_string(DatabaseBackend::Sqlite, update_sql))
            .await
            .internal()?;
        if result.rows_affected() == 0 {
            missing += 1;
        } else {
            restored += 1;
        }
    }

    Ok(UndoReport {
        job_id,
        restored,
        missing,
    })
}
