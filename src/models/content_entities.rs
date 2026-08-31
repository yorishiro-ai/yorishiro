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
/// Distinct from the generated `Model` because `Model` carries `embedding` (`Option<PgVector>`), which this API never returns: the search/embedding pipeline manages that column separately.
/// `created_by`/`updated_by` are `None` for entities touched by an unattributed API key.
///
/// `FromQueryResult` lets this also be built directly by [`select_record_columns`]'s SQLite path, which selects every column except `embedding` (a column that doesn't exist on that backend's `content_entities` table) rather than going through `Model`.
#[derive(Debug, Clone, Serialize, Deserialize, sea_orm::FromQueryResult)]
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

/// Restricts a `content_entities` query to every column `EntityRecord` needs, excluding `embedding`, on SQLite.
/// A no-op on PostgreSQL: that backend's `content_entities` table genuinely has an `embedding` column, so the ordinary `Model`-shaped query stays unrestricted there.
///
/// SQLite's `content_entities` table has no `embedding` column at all (see `migration/src/m20260829_000000_initial_schema.rs`'s `helpers::pg_only` around that column), so any query built from the generated `Entity`/`Model` unconditionally references it and fails with `no such column: content_entities.embedding` on that backend, whether or not the caller ever reads the field.
/// Every caller of `content_entities`'s query functions gets the same `EntityRecord` either way; only the column list sent to the database differs.
fn select_record_columns(conn: &impl ConnectionTrait, select: Select<Entity>) -> Select<Entity> {
    use super::_entities::content_entities::Column;

    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        select.select_only().columns([
            Column::Id,
            Column::WorkspaceId,
            Column::SchemaId,
            Column::SchemaVersion,
            Column::EntityType,
            Column::Data,
            Column::CreatedBy,
            Column::UpdatedBy,
            Column::CreatedAt,
            Column::UpdatedAt,
        ])
    } else {
        select
    }
}

/// Inserts `active` and returns the persisted row as an `EntityRecord`.
///
/// `ActiveModelTrait::insert` builds its return value by decoding a `content_entities::Model`, and SeaORM's `pgvector::Vector` `TryGetable` impl unconditionally errors on a SQLite row (`Vector unsupported by sqlx-sqlite`) regardless of whether the column has a value, so that path can never succeed on SQLite even though the insert itself would.
/// `Entity::insert(active).exec_without_returning` sidesteps the `Model` decode entirely; `select_record_columns`'s follow-up read (which already excludes `embedding`) then fetches the row `EntityRecord` actually needs.
///
/// `sqlite_generated_id` is called explicitly rather than relying on `ActiveModel`'s `before_save` hook: `Entity::insert(...).exec_without_returning(...)` doesn't call `ActiveModelBehavior::before_save` at all (only `ActiveModelTrait::insert`/`update` do), the same reason `tenancy::add_member` calls it directly for its own `on_conflict` insert.
async fn insert_and_fetch(
    conn: &impl ConnectionTrait,
    mut active: ActiveModel,
) -> Result<EntityRecord, YorishiroError> {
    use super::_entities::content_entities::Column;

    active.id = crate::db::sqlite_generated_id(conn, active.id);

    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        let ActiveValue::Set(id) = active.id else {
            return Err(YorishiroError::Internal(anyhow::anyhow!(
                "content_entities insert on SQLite has no id to fetch back"
            )));
        };
        Entity::insert(active)
            .exec_without_returning(conn)
            .await
            .internal()?;
        select_record_columns(conn, Entity::find().filter(Column::Id.eq(id)))
            .into_model::<EntityRecord>()
            .one(conn)
            .await
            .internal()?
            .ok_or_else(|| {
                YorishiroError::Internal(anyhow::anyhow!(
                    "content_entities row '{id}' was inserted but could not be read back"
                ))
            })
    } else {
        active.insert(conn).await.internal().map(EntityRecord::from)
    }
}

/// [`insert_and_fetch`]'s counterpart for an update: same reasoning (`ActiveModelTrait::update`
/// decodes the return value as a `content_entities::Model`, which fails on SQLite the same way
/// insert's does), same fix (route around the `Model` decode, re-fetch through
/// `select_record_columns`).
///
/// `active.id` must already name an existing row: unlike `insert_and_fetch`, this never generates one.
///
/// `ActiveModelTrait::update_without_returning` rather than the `Entity::update(...)` builder, because only the former still calls `before_save` (confirmed against `sea-orm` 2.0.2's source), so `updated_at` is stamped instead of silently staying stale.
/// The insert side has no such trait method, which is why `insert_and_fetch` calls `sqlite_generated_id` itself.
/// Raises `DbErr::RecordNotUpdated` on no match, exactly as `ActiveModelTrait::update` does, so `undo_job`'s match needs no change.
async fn update_and_fetch(
    conn: &impl ConnectionTrait,
    active: ActiveModel,
) -> Result<EntityRecord, YorishiroError> {
    use super::_entities::content_entities::Column;

    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        let id = match &active.id {
            ActiveValue::Unchanged(id) | ActiveValue::Set(id) => *id,
            ActiveValue::NotSet => {
                return Err(YorishiroError::Internal(anyhow::anyhow!(
                    "content_entities update on SQLite has no id to fetch back"
                )));
            }
        };
        active.update_without_returning(conn).await.internal()?;
        select_record_columns(conn, Entity::find().filter(Column::Id.eq(id)))
            .into_model::<EntityRecord>()
            .one(conn)
            .await
            .internal()?
            .ok_or_else(|| {
                YorishiroError::Internal(anyhow::anyhow!(
                    "content_entities row '{id}' was updated but could not be read back"
                ))
            })
    } else {
        active.update(conn).await.internal().map(EntityRecord::from)
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
///
/// `select_record_columns` isn't used here: `PaginatorTrait::count` builds its own `SELECT COUNT(*)` wrapping the query rather than projecting `Model`'s columns, but SeaORM still resolves that inner query against every column `Entity::find()` names, `embedding` included, which is what fails on SQLite.
/// Excluding `embedding` from the column list sidesteps that the same way the other functions in this file do.
pub async fn count(conn: &impl ConnectionTrait, workspace_id: Uuid) -> Result<i64, YorishiroError> {
    use super::_entities::content_entities::Column;

    let select = Entity::find().filter(Column::WorkspaceId.eq(workspace_id));
    select_record_columns(conn, select)
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
    insert_and_fetch(conn, active).await
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn get(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<EntityRecord, YorishiroError> {
    use super::_entities::content_entities::Column;

    let select = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(id));

    select_record_columns(conn, select)
        .into_model::<EntityRecord>()
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("entity '{id}' was not found")))
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
    use super::_entities::content_entities::Column;

    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let select = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.is_in(ids.iter().copied()));

    let rows = select_record_columns(conn, select)
        .into_model::<EntityRecord>()
        .all(conn)
        .await
        .internal()?;

    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
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
    update_and_fetch(conn, active).await
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

    select_record_columns(conn, select)
        .into_model::<EntityRecord>()
        .all(conn)
        .await
        .internal()
}

/// Fetches every entity for the workspace, with no pagination limit, for a full-workspace data export.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn export_all(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    use super::_entities::content_entities::Column;

    let select = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(Column::CreatedAt);

    select_record_columns(conn, select)
        .into_model::<EntityRecord>()
        .all(conn)
        .await
        .internal()
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
    /// Already on the active version.
    /// Nothing to do for these.
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

    let is_sqlite = conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite;

    for snap in &snapshots {
        // `snap.entity_id` is already workspace-scoped by the query above, so no extra filter is needed on the write.
        let active = ActiveModel {
            id: ActiveValue::Unchanged(snap.entity_id),
            data: ActiveValue::Set(snap.data.clone()),
            schema_id: ActiveValue::Set(snap.schema_id),
            schema_version: ActiveValue::Set(snap.schema_version),
            ..Default::default()
        };
        // `update_without_returning` for the reason `update_and_fetch` documents: `active.update`
        // decodes a full `Model`, which cannot succeed on SQLite. No read-back here, since this
        // only needs the `Ok`/`RecordNotUpdated` outcome rather than the row.
        // It still calls `before_save` (unlike the raw `Entity::update(...)` builder, which skips it and would leave `updated_at` stale), and still raises `DbErr::RecordNotUpdated` when no row matches, so the `match` below needs no branch of its own.
        let result = if is_sqlite {
            active.update_without_returning(conn).await.map(|_| ())
        } else {
            active.update(conn).await.map(|_| ())
        };
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

#[cfg(test)]
mod sqlite_tests {
    use crate::migration::{Migrator, MigratorTrait};
    use sea_orm::{ConnectionTrait, Database, Statement};

    use super::{
        CreateEntityInput, ListEntitiesQuery, count, create, delete, export_all, get, get_batch,
        list, update,
    };

    /// A fresh in-memory SQLite database, migrated, with one tenant/workspace/schema seeded via raw SQL (not through `tenancy`/`content_schemas`, to keep this test focused on `content_entities` itself).
    /// Mirrors `tenancy.rs`'s own `sqlite_db()` test helper.
    async fn seeded_sqlite_db() -> (sea_orm::DatabaseConnection, uuid::Uuid) {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("connect to in-memory sqlite");
        Migrator::up(&db, None).await.expect("run migrations");

        let tenant_id = uuid::Uuid::now_v7();
        let workspace_id = uuid::Uuid::now_v7();
        let schema_id = uuid::Uuid::now_v7();

        db.execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            "INSERT INTO identity_tenants (id, name) VALUES ($1, 'acme')",
            [tenant_id.into()],
        ))
        .await
        .expect("insert tenant");

        db.execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            "INSERT INTO identity_workspaces (id, tenant_id, name, status, max_entities) \
             VALUES ($1, $2, 'ws', 'active', NULL)",
            [workspace_id.into(), tenant_id.into()],
        ))
        .await
        .expect("insert workspace");

        let definition = serde_json::json!({
            "name": "notes",
            "entity_types": {
                "note": { "fields": {}, "required": [] }
            }
        });

        db.execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            "INSERT INTO content_schemas \
                (id, tenant_id, workspace_id, name, version, definition, status) \
             VALUES ($1, $2, $3, 'notes', 1, $4, 'active')",
            [
                schema_id.into(),
                tenant_id.into(),
                workspace_id.into(),
                definition.to_string().into(),
            ],
        ))
        .await
        .expect("insert schema");

        (db, workspace_id)
    }

    /// Exercises all eight query functions against SQLite in one pass: `count`, `get`, `get_batch`,
    /// `list`, `export_all`, `create`, `update` and `delete`.
    #[tokio::test]
    async fn content_entities_crud_on_sqlite() {
        let (db, workspace_id) = seeded_sqlite_db().await;

        let input = CreateEntityInput {
            schema_name: "notes".into(),
            entity_type: "note".into(),
            data: serde_json::json!({"title": "first"}),
        };
        let created = create(&db, workspace_id, input, None)
            .await
            .expect("create");
        assert_eq!(created.data["title"], "first");

        let fetched = get(&db, workspace_id, created.id).await.expect("get");
        assert_eq!(fetched.id, created.id);

        let batch = get_batch(&db, workspace_id, &[created.id])
            .await
            .expect("get_batch");
        assert_eq!(batch.len(), 1);
        assert!(batch.contains_key(&created.id));

        let listed = list(&db, workspace_id, ListEntitiesQuery::default())
            .await
            .expect("list");
        assert_eq!(listed.len(), 1);

        let exported = export_all(&db, workspace_id).await.expect("export_all");
        assert_eq!(exported.len(), 1);

        let counted = count(&db, workspace_id).await.expect("count");
        assert_eq!(counted, 1);

        let updated = update(
            &db,
            workspace_id,
            created.id,
            serde_json::json!({"title": "second"}),
            None,
        )
        .await
        .expect("update");
        assert_eq!(updated.data["title"], "second");
        assert!(
            updated.updated_at > created.updated_at,
            "updated_at should advance on update: created {:?}, updated {:?}",
            created.updated_at,
            updated.updated_at
        );

        delete(&db, workspace_id, created.id).await.expect("delete");

        let after_delete = count(&db, workspace_id).await.expect("count after delete");
        assert_eq!(after_delete, 0);
    }

    /// `undo_job` calls `ActiveModel::update(conn)` directly rather than going through `content_entities::update`, so it carries its own SQLite branch instead of inheriting `update_and_fetch`'s.
    /// Guards both outcomes its `match` distinguishes: a snapshot whose entity still exists (`restored`) and one whose entity was deleted since (`missing`, via `DbErr::RecordNotUpdated`).
    #[tokio::test]
    async fn undo_job_restores_and_counts_a_missing_entity_on_sqlite() {
        let (db, workspace_id) = seeded_sqlite_db().await;

        let input = CreateEntityInput {
            schema_name: "notes".into(),
            entity_type: "note".into(),
            data: serde_json::json!({"title": "original"}),
        };
        let created = create(&db, workspace_id, input, None)
            .await
            .expect("create");

        let schema_id = super::get(&db, workspace_id, created.id)
            .await
            .expect("get")
            .schema_id;
        let job_id = uuid::Uuid::now_v7();

        // A snapshot for the entity that still exists: `undo_job` should restore it.
        let existing_snapshot_id = uuid::Uuid::now_v7();
        db.execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            "INSERT INTO content_entity_snapshots \
                (id, job_id, workspace_id, entity_id, schema_id, schema_version, data) \
             VALUES ($1, $2, $3, $4, $5, 1, $6)",
            [
                existing_snapshot_id.into(),
                job_id.into(),
                workspace_id.into(),
                created.id.into(),
                schema_id.into(),
                serde_json::json!({"title": "restored"}).to_string().into(),
            ],
        ))
        .await
        .expect("insert snapshot for the existing entity");

        // A snapshot for an entity that no longer exists: `undo_job` should count it as missing,
        // not fail the whole batch.
        let deleted_entity_id = uuid::Uuid::now_v7();
        let missing_snapshot_id = uuid::Uuid::now_v7();
        db.execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            "INSERT INTO content_entity_snapshots \
                (id, job_id, workspace_id, entity_id, schema_id, schema_version, data) \
             VALUES ($1, $2, $3, $4, $5, 1, $6)",
            [
                missing_snapshot_id.into(),
                job_id.into(),
                workspace_id.into(),
                deleted_entity_id.into(),
                schema_id.into(),
                serde_json::json!({"title": "gone"}).to_string().into(),
            ],
        ))
        .await
        .expect("insert snapshot for the deleted entity");

        let report = super::undo_job(&db, workspace_id, job_id)
            .await
            .expect("undo_job");
        assert_eq!(report.restored, 1);
        assert_eq!(report.missing, 1);

        let restored = super::get(&db, workspace_id, created.id)
            .await
            .expect("get after undo");
        assert_eq!(restored.data["title"], "restored");
    }
}
