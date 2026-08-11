use sea_query::extension::postgres::PgExpr;
use sea_query::{Alias, Asterisk, Expr, Func, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde_json::Value;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use crate::error::{ResultExt, ValidationDetail, YorishiroError};
use crate::metaschema;
use crate::repositories::schemas;

pub use crate::models::entities::*;

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

/// Represents where a validation error occurred as a JSON Pointer. For `required`
/// violations, `instance_path()` alone only points at the containing object and doesn't
/// say which property is missing, so the missing property name is appended.
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
/// Reuses `entity_type_to_json_schema`'s schema as-is so validation logic isn't duplicated
/// between entities and the MCP inputSchema.
///
/// `pub` (rather than private) only so the crate-root integration test in `tests/` can call
/// it directly; `#[doc(hidden)]` keeps it out of the public API docs since it isn't meant for
/// external callers.
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
/// `NULL` means unlimited, which is the default so self-hosted deployments are never capped
/// unless an operator explicitly sets a limit. The app role only has SELECT on
/// `identity.workspaces`, which is enough to read this column without needing write access to
/// the control-plane schema.
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

/// Counts how many entities a workspace holds, for both quota enforcement (`create`, above)
/// and workspace-detail summaries.
pub async fn count(conn: &mut PgConnection, workspace_id: Uuid) -> Result<i64, YorishiroError> {
    let (sql, values) = Query::select()
        .expr(Func::count(Expr::col(Asterisk)))
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);
    let (count,): (i64,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .internal()?;
    Ok(count)
}

/// Creates a new entity: resolves the schema name to its currently active schema, checks
/// that the entity_type exists in that version, validates `data`, and persists the result.
/// `created_by` is the acting user's ID (from `AuthContext::user_id`), or `None` for an
/// unattributed service/automation API key.
///
/// The quota check and insert are serialized with a workspace-scoped advisory lock: without
/// it, concurrent creates could each read a count under `max_entities` and both insert,
/// overshooting the cap (the same TOCTOU that `create_schema` guards against for schemas).
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

    // Before resolving the schema, so an empty workspace is told it is empty. Resolving first
    // would report the schema name as not found, which reads as a typo rather than as "nothing
    // has been defined here yet".
    if crate::repositories::tenancy::is_schema_pending(&mut tx, workspace_id).await? {
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

pub async fn get(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<EntityRecord, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(entity_columns())
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Entities::Id).eq(id))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, EntityRecord, _>(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("entity '{id}' was not found")))
}

/// Fully replaces an existing entity's `data`. Validation is done against the schema
/// version the entity was actually created with (i.e. the row `entities.schema_id` points
/// to), so existing entities don't silently break compatibility even if the active version
/// has since moved on.
/// `updated_by` is the acting user's ID, or `None` for an unattributed service/automation
/// API key -- this overwrites whatever `updated_by` the previous update (if any) left behind.
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
#[path = "../../../tests/repositories/entities/mod.rs"]
mod tests;

/// Reports how `entity_id` stands against the active version of its schema.
///
/// Lazy migration means an entity keeps validating against the version it was created with, so
/// a field added later is simply absent from it. This distinguishes that from a field its
/// author left blank: the entity's own definition is compared against the active one, and the
/// fields only the active one defines are returned.
///
/// An entity already on the active version reports no missing fields, and neither does one
/// whose newer version only altered fields it already carries.
pub async fn drift(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    entity_id: Uuid,
) -> Result<EntityDrift, YorishiroError> {
    let entity = get(conn, workspace_id, entity_id).await?;
    let own = schemas::get_by_id(conn, workspace_id, entity.schema_id).await?;
    let active = schemas::get_active_schema(conn, workspace_id, &own.definition.name).await?;

    // The entity's own type definition may be absent from the active version -- the type was
    // dropped. Nothing is "missing" in that case; the whole type is, which the version numbers
    // already say.
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
/// Reads only. The counting is done in one query per entity type rather than one per entity:
/// a workspace can hold far more entities than it holds versions, and the answer is the same.
pub async fn migration_dry_run(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    schema_name: &str,
) -> Result<MigrationDryRun, YorishiroError> {
    let active = schemas::get_active_schema(conn, workspace_id, schema_name).await?;

    // (entity_type, schema_id, count) for everything under this schema name, whatever version.
    // Grouping by schema_id is what keeps this proportional to the number of versions in use
    // rather than to the number of entities.
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
        // Optional additions are not counted: those entities are valid as they stand, and
        // reporting them as work to do would inflate the number an operator acts on.
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
            // The type is gone from the active version, or was never in the old one. Neither
            // is a field to fill in.
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
