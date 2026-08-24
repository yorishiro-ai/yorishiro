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

pub type ContentEntities = Entity;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    /// Stamps `updated_at` on every update whose caller didn't already set it explicitly.
    /// Checks `!is_set()` rather than `is_unchanged()`: an `ActiveModel` built with `..Default::default()` leaves untouched fields `NotSet`, not `Unchanged`, and `is_unchanged()` only matches the latter.
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
/// Distinct from the generated `Model` because `Model` carries `embedding` (`Option<PgVector>`), which this API never returns: the search/embedding pipeline manages that column separately.
/// `created_by`/`updated_by` are `None` for entities touched by an unattributed API key.
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
    /// Entities keep the version they were written against, so this selects the entities a
    /// given version produced, not the ones that would validate against it today.
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

/// Checks the workspace's `max_entities` cap before an insert.
/// `NULL` means unlimited, the default for self-hosted deployments.
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
    use super::_entities::content_entities::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .count(conn)
        .await
        .internal()
        .map(|n| n as i64)
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

/// `query.filter` (JSONB containment, `data @> filter`) is the one condition here `ColumnTrait` can't express (`ColumnTrait::contains` builds a `LIKE '%...%'`, unrelated to Postgres's `@>` operator), so it's added as a raw `Expr::cust_with_values` condition instead.
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

/// Fetches every entity for the workspace, with no pagination limit, for a full-workspace data export.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn export_all(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    use super::_entities::content_entities::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(Column::CreatedAt)
        .all(conn)
        .await
        .internal()
        .map(|rows| rows.into_iter().map(EntityRecord::from).collect())
}

/// How one entity stands relative to the active version of its schema.
///
/// Entities are migrated lazily: a schema gaining a version does not rewrite rows written against earlier ones.
/// This exists so a reader can tell whether a field is absent because nobody filled it in or because it did not exist when the entity was written.
#[derive(Debug, Clone, Serialize)]
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
#[derive(Debug, Clone, Serialize)]
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
#[derive(Debug, Clone, Serialize)]
pub struct MigrationDryRun {
    pub schema_name: String,
    /// The version everything would be brought to.
    pub active_version: i32,
    pub total_entities: i64,
    /// Already on the active version. Nothing to do for these.
    pub current: i64,
    /// On an older version, but missing no field the active version requires: they validate as they stand and only their version marker is behind.
    pub behind_but_valid: i64,
    /// On an older version and missing at least one field the active version requires.
    /// These are what a migration has to fill in.
    pub needs_values: i64,
    /// Per entity type, so an operator can see whether the work is spread or concentrated.
    pub by_entity_type: Vec<DryRunByType>,
}

#[derive(Debug, Clone, Serialize)]
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
    #[derive(Debug, sea_orm::FromQueryResult)]
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
#[derive(Debug, Clone, Serialize, sea_orm::FromQueryResult)]
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
#[derive(Debug, Clone, Serialize)]
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
            sea_orm::DatabaseBackend::Postgres,
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
        match active.update(conn).await {
            Ok(_) => restored += 1,
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
