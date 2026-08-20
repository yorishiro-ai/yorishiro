use chrono::{DateTime, Utc};
use sea_query::extension::postgres::PgExpr;
use sea_query::{Alias, Asterisk, Expr, Func, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Connection, PgConnection};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ResultExt, ValidationDetail, YorishiroError};
use crate::metaschema;
use crate::models::schemas;

/// A row in the `entities` table.
/// `embedding` is managed separately by the search/embedding pipeline, so this module's CRUD doesn't touch it.
/// `created_by`/ `updated_by` are `None` for entities touched by an unattributed (service/automation) API key, since there's no user to record.
/// `Deserialize` is derived so this can be read back from a JSONL export (see `models::import`); import treats `id`, `schema_version`, `created_at`/`updated_at` as informational only (a fresh row is always inserted).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct EntityRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub schema_id: Uuid,
    pub schema_version: i32,
    pub entity_type: String,
    #[schema(value_type = Object)]
    pub data: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub updated_by: Option<Uuid>,
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

/// How one entity stands relative to the active version of its schema.
///
/// Entities are migrated lazily: a schema gaining a version does not rewrite the rows written against earlier ones, and an update validates against the version the entity was created with.
/// That is deliberate: it is what stops a schema change from invalidating stored data.
/// But it leaves a reader unable to tell whether a field is absent because nobody filled it in or because it did not exist when the entity was written.
#[derive(Debug, Clone, Serialize, ToSchema)]
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
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DriftField {
    pub name: String,
    /// The field's type in the active version, so a caller can tell what would go there.
    /// Serializes to the same spelling the schema uses.
    pub r#type: crate::metaschema::FieldTypeName,
    /// Whether the active version marks it required.
    /// A required field an old entity lacks is the case worth surfacing: the entity is valid under its own version and would not be under the current one.
    pub required: bool,
}

/// What a batch migration would find, without doing it.
///
/// Migration is lazy: an entity keeps validating against the version it was written with, so a workspace accumulates entities spread across versions.
/// This counts them before anything is touched, because the useful question before a migration is how much of the corpus it would have to fill in: a number that decides whether defaults suffice or whether the work needs a person.
#[derive(Debug, Clone, Serialize, ToSchema)]
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
    /// These are what a migration has to fill in, and what mode A's defaults or mode B's inference would be for.
    pub needs_values: i64,
    /// Per entity type, so an operator can see whether the work is spread or concentrated.
    pub by_entity_type: Vec<DryRunByType>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DryRunByType {
    pub entity_type: String,
    pub behind: i64,
    pub needs_values: i64,
    /// The required fields those entities lack, so the report names the work rather than only counting it.
    pub missing_required: Vec<String>,
}

/// An entity's data as it stood before something overwrote it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct EntitySnapshot {
    pub id: Uuid,
    /// Groups the snapshots taken by one operation, so a batch is undone as a batch.
    pub job_id: Uuid,
    pub entity_id: Uuid,
    pub schema_id: Uuid,
    pub schema_version: i32,
    #[schema(value_type = Object)]
    pub data: Value,
    pub created_at: DateTime<Utc>,
}

/// What undoing a job put back.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UndoReport {
    pub job_id: Uuid,
    /// Entities restored to the data they held before.
    pub restored: i64,
    /// Snapshots whose entity no longer exists.
    /// Counted rather than treated as an error: a batch partially undone leaves a workspace in a state nobody chose, and an entity deleted since is not a reason to refuse the rest.
    pub missing: i64,
}

/// What filling defaults did.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FillDefaultsReport {
    /// Groups the snapshots taken, so this run can be undone as one.
    pub job_id: Uuid,
    pub schema_name: String,
    /// Entities that gained at least one value.
    pub filled: i64,
    /// Entities that needed a value the active version defines no default for.
    /// Left untouched and counted, because inventing one would be worse than leaving the field absent: a value nobody chose is indistinguishable from one someone did.
    pub skipped_no_default: i64,
    /// The fields those entities still lack.
    pub still_missing: Vec<String>,
}

#[derive(Iden)]
enum Entities {
    Table,
    Id,
    WorkspaceId,
    SchemaId,
    SchemaVersion,
    EntityType,
    Data,
    CreatedAt,
    UpdatedAt,
    CreatedBy,
    UpdatedBy,
}

#[derive(Iden)]
enum Workspaces {
    Table,
    Id,
    MaxEntities,
}

fn entity_columns() -> [Entities; 10] {
    [
        Entities::Id,
        Entities::WorkspaceId,
        Entities::SchemaId,
        Entities::SchemaVersion,
        Entities::EntityType,
        Entities::Data,
        Entities::CreatedAt,
        Entities::UpdatedAt,
        Entities::CreatedBy,
        Entities::UpdatedBy,
    ]
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
///
/// `pub` (rather than private) only so the crate-root integration test in `tests/` can call it directly; `#[doc(hidden)]` keeps it out of the public API docs since it isn't meant for external callers.
#[doc(hidden)]
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
/// The app role only has SELECT on `identity.workspaces`, which is enough to read this column without needing write access to the control-plane schema.
async fn check_entity_quota(
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    let (sql, values) = Query::select()
        .column(Workspaces::MaxEntities)
        .from((Alias::new("identity"), Workspaces::Table))
        .and_where(Expr::col(Workspaces::Id).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);
    let max_entities: Option<i32> = sqlx::query_scalar_with(&sql, values)
        .fetch_optional(&mut *conn)
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
pub async fn count<C>(conn: &mut C, workspace_id: Uuid) -> Result<i64, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    (i64,): for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let (sql, values) = Query::select()
        .expr(Func::count(Expr::col(Asterisk)))
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .build_sqlx(C::builder());
    let (count,): (i64,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .internal()?;
    Ok(count)
}

/// Creates a new entity: resolves the schema name to its currently active schema, checks that the entity_type exists in that version, validates `data`, and persists the result.
/// `created_by` is the acting user's ID (from `AuthContext::user_id`), or `None` for an unattributed service/automation API key.
///
/// The quota check and insert are serialized with a workspace-scoped advisory lock: without it, concurrent creates could each read a count under `max_entities` and both insert, overshooting the cap (the same TOCTOU that `create_schema` guards against for schemas).
pub async fn create(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    input: CreateEntityInput,
    created_by: Option<Uuid>,
) -> Result<EntityRecord, YorishiroError> {
    let mut tx = conn.begin().await.internal()?;

    crate::db::lock_for_update(&mut tx, &workspace_id.to_string())
        .await
        .internal()?;

    check_entity_quota(&mut tx, workspace_id).await?;

    // Before resolving the schema, so an empty workspace is told it is empty.
    // Resolving first would report the schema name as not found, which reads as a typo rather than as "nothing has been defined here yet".
    if crate::models::tenancy::is_schema_pending(&mut tx, workspace_id).await? {
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

    let schema = schemas::get_active_schema(&mut tx, workspace_id, &input.schema_name).await?;
    let entity_type_def = resolve_entity_type(&schema.definition, &input.entity_type)?;
    validate_data(entity_type_def, &input.data)?;

    let (sql, values) = Query::insert()
        .into_table((Alias::new("content"), Entities::Table))
        .columns([
            Entities::WorkspaceId,
            Entities::SchemaId,
            Entities::SchemaVersion,
            Entities::EntityType,
            Entities::Data,
            Entities::CreatedBy,
        ])
        .values_panic([
            workspace_id.into(),
            schema.id.into(),
            schema.version.into(),
            input.entity_type.into(),
            input.data.into(),
            created_by.into(),
        ])
        .returning(Query::returning().columns(entity_columns()))
        .build_sqlx(PostgresQueryBuilder);

    let row: EntityRecord = sqlx::query_as_with::<_, EntityRecord, _>(&sql, values)
        .fetch_one(&mut *tx)
        .await
        .internal()?;

    tx.commit().await.internal()?;

    Ok(row)
}

pub async fn get<C>(
    conn: &mut C,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<EntityRecord, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    EntityRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let (sql, values) = Query::select()
        .columns(entity_columns())
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Entities::Id).eq(id))
        .build_sqlx(C::builder());

    sqlx::query_as_with::<_, EntityRecord, _>(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("entity '{id}' was not found")))
}

/// Fully replaces an existing entity's `data`.
/// Validation is done against the schema version the entity was actually created with (i.e. the row `entities.schema_id` points to), so existing entities don't silently break compatibility even if the active version has since moved on.
/// `updated_by` is the acting user's ID, or `None` for an unattributed service/automation API key: this overwrites whatever `updated_by` the previous update (if any) left behind.
pub async fn update(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    id: Uuid,
    data: Value,
    updated_by: Option<Uuid>,
) -> Result<EntityRecord, YorishiroError> {
    let existing = get(conn, workspace_id, id).await?;
    let schema = schemas::get_by_id(conn, workspace_id, existing.schema_id).await?;
    let entity_type_def = resolve_entity_type(&schema.definition, &existing.entity_type)?;
    validate_data(entity_type_def, &data)?;

    let (sql, values) = Query::update()
        .table((Alias::new("content"), Entities::Table))
        .value(Entities::Data, data)
        .value(Entities::UpdatedAt, Expr::cust("now()"))
        .value(Entities::UpdatedBy, updated_by)
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Entities::Id).eq(id))
        .returning(Query::returning().columns(entity_columns()))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, EntityRecord, _>(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("entity '{id}' was not found")))
}

pub async fn delete(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<(), YorishiroError> {
    let (sql, values) = Query::delete()
        .from_table((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Entities::Id).eq(id))
        .build_sqlx(PostgresQueryBuilder);

    let result = sqlx::query_with(&sql, values)
        .execute(&mut *conn)
        .await
        .internal()?;

    if result.rows_affected() == 0 {
        Err(YorishiroError::not_found(format!(
            "entity '{id}' was not found"
        )))
    } else {
        Ok(())
    }
}

pub async fn list(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    query: ListEntitiesQuery,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    let limit = query.limit.clamp(1, 200);
    let offset = query.offset.max(0);

    let mut builder = Query::select();
    builder
        .columns(entity_columns())
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id));
    if let Some(entity_type) = query.entity_type {
        builder.and_where(Expr::col(Entities::EntityType).eq(entity_type));
    }
    if let Some(filter) = query.filter {
        builder.and_where(Expr::col(Entities::Data).contains(filter));
    }
    if let Some(schema_version) = query.schema_version {
        builder.and_where(Expr::col(Entities::SchemaVersion).eq(schema_version));
    }
    builder
        .order_by(Entities::CreatedAt, Order::Desc)
        .limit(limit as u64)
        .offset(offset as u64);
    let (sql, values) = builder.build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, EntityRecord, _>(&sql, values)
        .fetch_all(&mut *conn)
        .await
        .internal()
}

/// Fetches every entity for the tenant, with no pagination limit, for a full-tenant export.
pub async fn export_all(
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(entity_columns())
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .order_by(Entities::CreatedAt, Order::Asc)
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, EntityRecord, _>(&sql, values)
        .fetch_all(&mut *conn)
        .await
        .internal()
}

#[cfg(test)]
#[path = "../../../tests/models/entities/mod.rs"]
mod tests;

/// Reports how `entity_id` stands against the active version of its schema.
///
/// Lazy migration means an entity keeps validating against the version it was created with, so a field added later is simply absent from it.
/// This distinguishes that from a field its author left blank: the entity's own definition is compared against the active one, and the fields only the active one defines are returned.
///
/// An entity already on the active version reports no missing fields, and neither does one whose newer version only altered fields it already carries.
pub async fn drift(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    entity_id: Uuid,
) -> Result<EntityDrift, YorishiroError> {
    let entity = get(conn, workspace_id, entity_id).await?;
    let own = schemas::get_by_id(conn, workspace_id, entity.schema_id).await?;
    let active = schemas::get_active_schema(conn, workspace_id, &own.definition.name).await?;

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

/// Counts what a batch migration to `schema_name`'s active version would face.
///
/// Reads only.
/// The counting is done in one query per entity type rather than one per entity:
/// a workspace can hold far more entities than it holds versions, and the answer is the same.
pub async fn migration_dry_run(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    schema_name: &str,
) -> Result<MigrationDryRun, YorishiroError> {
    let active = schemas::get_active_schema(conn, workspace_id, schema_name).await?;

    // (entity_type, schema_id, count) for everything under this schema name, whatever version.
    // Grouping by schema_id is what keeps this proportional to the number of versions in use rather than to the number of entities.
    let rows: Vec<(String, Uuid, i64)> = sqlx::query_as(
        "SELECT e.entity_type, e.schema_id, count(*) \
         FROM content.entities e \
         JOIN content.schemas s ON s.id = e.schema_id \
         WHERE e.workspace_id = $1 AND s.name = $2 \
         GROUP BY e.entity_type, e.schema_id",
    )
    .bind(workspace_id)
    .bind(schema_name)
    .fetch_all(&mut *conn)
    .await
    .internal()?;

    let mut total = 0i64;
    let mut current = 0i64;
    let mut behind_valid = 0i64;
    let mut needs_values = 0i64;
    let mut by_type: std::collections::BTreeMap<String, DryRunByType> =
        std::collections::BTreeMap::new();

    // Each distinct old version is fetched once, not once per entity.
    let mut definitions: std::collections::HashMap<Uuid, crate::metaschema::MetaSchemaDefinition> =
        std::collections::HashMap::new();

    for (entity_type, schema_id, count) in rows {
        total += count;

        if schema_id == active.id {
            current += count;
            continue;
        }

        let old = match definitions.get(&schema_id) {
            Some(def) => def.clone(),
            None => {
                let record = schemas::get_by_id(conn, workspace_id, schema_id).await?;
                definitions.insert(schema_id, record.definition.clone());
                record.definition
            }
        };

        // Required in the active version, absent from the version these were written with.
        // Optional additions are not counted: those entities are valid as they stand, and reporting them as work to do would inflate the number an operator acts on.
        let missing: Vec<String> = match (
            active.definition.entity_types.get(&entity_type),
            old.entity_types.get(&entity_type),
        ) {
            (Some(active_type), Some(old_type)) => active_type
                .fields
                .iter()
                .filter(|(name, def)| def.required && !old_type.fields.contains_key(*name))
                .map(|(name, _)| name.clone())
                .collect(),
            // The type is gone from the active version, or was never in the old one.
            // Neither is a field to fill in.
            _ => Vec::new(),
        };

        let entry = by_type
            .entry(entity_type.clone())
            .or_insert_with(|| DryRunByType {
                entity_type,
                behind: 0,
                needs_values: 0,
                missing_required: Vec::new(),
            });
        entry.behind += count;

        if missing.is_empty() {
            behind_valid += count;
        } else {
            needs_values += count;
            entry.needs_values += count;
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

/// Records what `entity_id` holds now, tagged with `job_id`.
///
/// Called before an overwrite.
/// Taking the image from the row rather than from the caller means it is what the database actually holds, not what the caller believed it held.
pub async fn snapshot(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    entity_id: Uuid,
    job_id: Uuid,
) -> Result<(), YorishiroError> {
    // One statement: reading then writing would leave a window in which a concurrent update slips between, and the image would be of a state that no longer existed when it was taken.
    let affected = sqlx::query(
        "INSERT INTO content.entity_snapshots \
             (job_id, workspace_id, entity_id, schema_id, schema_version, data) \
         SELECT $3, workspace_id, id, schema_id, schema_version, data \
         FROM content.entities \
         WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(entity_id)
    .bind(job_id)
    .execute(&mut *conn)
    .await
    .internal()?
    .rows_affected();

    if affected == 0 {
        return Err(YorishiroError::not_found(format!(
            "entity '{entity_id}' was not found"
        )));
    }
    Ok(())
}

/// The snapshots taken by one job, newest first.
pub async fn snapshots_for_job(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<Vec<EntitySnapshot>, YorishiroError> {
    sqlx::query_as::<_, EntitySnapshot>(
        "SELECT id, job_id, entity_id, schema_id, schema_version, data, created_at \
         FROM content.entity_snapshots \
         WHERE workspace_id = $1 AND job_id = $2 \
         ORDER BY created_at DESC",
    )
    .bind(workspace_id)
    .bind(job_id)
    .fetch_all(&mut *conn)
    .await
    .internal()
}

/// Puts every entity in `job_id` back to what it held before.
///
/// All in one transaction: a half-undone batch is a state nobody asked for, and worse than either end of it.
/// An entity deleted since the snapshot is counted rather than failed:
/// refusing the whole undo because one row is gone would leave the rest wrong.
pub async fn undo_job(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<UndoReport, YorishiroError> {
    let mut tx = conn.begin().await.internal()?;

    let snapshots = sqlx::query_as::<_, EntitySnapshot>(
        "SELECT id, job_id, entity_id, schema_id, schema_version, data, created_at \
         FROM content.entity_snapshots \
         WHERE workspace_id = $1 AND job_id = $2 \
         ORDER BY created_at ASC",
    )
    .bind(workspace_id)
    .bind(job_id)
    .fetch_all(&mut *tx)
    .await
    .internal()?;

    if snapshots.is_empty() {
        return Err(YorishiroError::not_found(format!(
            "no snapshots for job '{job_id}'"
        )));
    }

    let mut restored = 0i64;
    let mut missing = 0i64;

    for snapshot in &snapshots {
        // schema_id and schema_version go back too: an undo that restored the data but left the entity claiming a newer version would leave it validating against a definition its data no longer matches.
        let affected = sqlx::query(
            "UPDATE content.entities \
             SET data = $3, schema_id = $4, schema_version = $5, updated_at = now() \
             WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(snapshot.entity_id)
        .bind(&snapshot.data)
        .bind(snapshot.schema_id)
        .bind(snapshot.schema_version)
        .execute(&mut *tx)
        .await
        .internal()?
        .rows_affected();

        if affected == 0 {
            missing += 1;
        } else {
            restored += 1;
        }
    }

    // The snapshots go with the undo.
    // Keeping them would let the same job be undone twice, the second time restoring what the first already put back over whatever came after.
    sqlx::query("DELETE FROM content.entity_snapshots WHERE workspace_id = $1 AND job_id = $2")
        .bind(workspace_id)
        .bind(job_id)
        .execute(&mut *tx)
        .await
        .internal()?;

    tx.commit().await.internal()?;

    Ok(UndoReport {
        job_id,
        restored,
        missing,
    })
}

/// Fills fields that the active schema version defines a `default` for, into entities written before those fields existed.
///
/// The entity keeps its own schema version.
/// Filling a value is not a migration to the newer definition: it adds data the entity was always allowed to hold, validated against the version the entity already claims.
/// Moving an entity between versions is a separate question and not this one.
///
/// Every entity touched is snapshotted under one `job_id` first, so the whole run can be put back with [`undo_job`].
///
/// Fields with no `default` are left alone and reported.
/// Inventing a value would be worse than an absent field: once written, a value nobody chose looks exactly like one someone did.
pub async fn fill_defaults(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    schema_name: &str,
    job_id: Uuid,
) -> Result<FillDefaultsReport, YorishiroError> {
    let active = schemas::get_active_schema(conn, workspace_id, schema_name).await?;

    let mut filled = 0i64;
    let mut skipped = 0i64;
    let mut still_missing: Vec<String> = Vec::new();

    // One transaction: a half-filled run leaves a workspace in a state nobody asked for, and the snapshots would describe a rollback point that was never a whole state.
    let mut tx = conn.begin().await.internal()?;

    // Drop the images that have aged out before writing this job's.
    // Here rather than on a timer because nothing in this crate runs on one: a sweeper would be the first thing of its kind, and a workspace that never migrates has nothing to sweep.
    prune_snapshots(&mut tx, workspace_id).await?;

    let rows: Vec<(Uuid, String, Value)> = sqlx::query_as(
        "SELECT e.id, e.entity_type, e.data \
         FROM content.entities e \
         JOIN content.schemas s ON s.id = e.schema_id \
         WHERE e.workspace_id = $1 AND s.name = $2 AND e.schema_id <> $3",
    )
    .bind(workspace_id)
    .bind(schema_name)
    .bind(active.id)
    .fetch_all(&mut *tx)
    .await
    .internal()?;

    for (entity_id, entity_type, data) in rows {
        let Some(type_def) = active.definition.entity_types.get(&entity_type) else {
            // The active version dropped this type.
            // Nothing to fill against.
            continue;
        };
        let Some(object) = data.as_object() else {
            continue;
        };

        let mut updated = object.clone();
        let mut changed = false;
        let mut missing_here = Vec::new();

        for (name, field) in &type_def.fields {
            if object.contains_key(name) {
                continue;
            }
            match &field.default {
                Some(value) => {
                    updated.insert(name.clone(), value.clone());
                    changed = true;
                }
                None if field.required => missing_here.push(name.clone()),
                None => {}
            }
        }

        if changed {
            snapshot_in(&mut tx, workspace_id, entity_id, job_id).await?;

            let (sql, values) = Query::update()
                .table((Alias::new("content"), Entities::Table))
                .values([
                    (Entities::Data, Value::Object(updated).into()),
                    (Entities::UpdatedAt, Expr::current_timestamp().into()),
                ])
                .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
                .and_where(Expr::col(Entities::Id).eq(entity_id))
                .build_sqlx(PostgresQueryBuilder);
            sqlx::query_with(&sql, values)
                .execute(&mut *tx)
                .await
                .internal()?;
            filled += 1;
        }

        if !missing_here.is_empty() {
            skipped += 1;
            for name in missing_here {
                if !still_missing.contains(&name) {
                    still_missing.push(name);
                }
            }
        }
    }

    tx.commit().await.internal()?;

    Ok(FillDefaultsReport {
        job_id,
        schema_name: schema_name.to_string(),
        filled,
        skipped_no_default: skipped,
        still_missing,
    })
}

/// How long a batch migration stays undoable.
///
/// `YORISHIRO_SNAPSHOT_RETENTION_DAYS` (default 30); `0` keeps every image forever.
/// Left unbounded, a workspace that migrates repeatedly accumulates before-images faster than it holds entities: every run writes one row per entity it touches, and only an undo takes them away again.
///
/// The guarantee this buys is stated in days, not in rows: **a batch migration can be undone for this many days.** Past that its images are gone and `undo_job` answers `NotFound`, the same as for a job that never ran: an expired window is indistinguishable from no window, and that is what the setting means rather than a fault to guard against.
/// Read as `i32` because that is what `make_interval(days => …)` takes.
/// A wider parse would let a value above `i32::MAX` wrap negative, and a negative interval puts the cutoff in the *future*: the sweep would then delete the images it exists to keep.
/// Anything unparseable, out of range included, falls back to the default rather than being clamped: a retention of six million years is a typo, and honouring the nearest legal value would hide it.
fn snapshot_retention_days() -> i32 {
    std::env::var("YORISHIRO_SNAPSHOT_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30)
}

/// Deletes this workspace's before-images older than the retention window.
///
/// Workspace-scoped so the delete stays inside RLS, and `yorishiro_app` already holds DELETE on the table.
/// Runs once per job rather than once per entity: the sweep is the same work either way, and a thousand-entity migration should not pay for it a thousand times.
async fn prune_snapshots(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    let days = snapshot_retention_days();
    if days <= 0 {
        return Ok(());
    }

    // `make_interval` rather than a formatted string: the number reaches Postgres as a bound parameter, so a retention value out of the environment is never concatenated into SQL.
    sqlx::query(
        "DELETE FROM content.entity_snapshots \
         WHERE workspace_id = $1 AND created_at < now() - make_interval(days => $2)",
    )
    .bind(workspace_id)
    .bind(days)
    .execute(&mut **tx)
    .await
    .internal()?;

    Ok(())
}

/// [`snapshot`] against an open transaction, so the image and the write it protects commit or roll back together.
async fn snapshot_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    entity_id: Uuid,
    job_id: Uuid,
) -> Result<(), YorishiroError> {
    sqlx::query(
        "INSERT INTO content.entity_snapshots \
             (job_id, workspace_id, entity_id, schema_id, schema_version, data) \
         SELECT $3, workspace_id, id, schema_id, schema_version, data \
         FROM content.entities \
         WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(entity_id)
    .bind(job_id)
    .execute(&mut **tx)
    .await
    .internal()?;
    Ok(())
}
