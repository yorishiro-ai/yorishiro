use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

pub use super::_entities::content_entities::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, ValidationDetail, YorishiroError};
use crate::metaschema;

pub type ContentEntities = Entity;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if !insert && self.updated_at.is_unchanged() {
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

/// A row in the `content_entities` table, for the RLS-scoped request path.
/// `embedding` is managed separately by the search/embedding pipeline, so this module's CRUD doesn't touch it.
/// `created_by`/`updated_by` are `None` for entities touched by an unattributed (service/automation) API key, since there's no user to record.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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

pub struct CreateEntityInput {
    pub schema_name: String,
    pub entity_type: String,
    pub data: Value,
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
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    let max_entities: Option<i32> =
        sqlx::query_scalar("SELECT max_entities FROM identity_workspaces WHERE id = $1")
            .bind(workspace_id)
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
pub async fn count(conn: &mut PgConnection, workspace_id: Uuid) -> Result<i64, YorishiroError> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM content_entities WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&mut *conn)
            .await
            .internal()?;
    Ok(count)
}

/// Creates a new entity: resolves the schema name to its currently active schema, checks that the entity_type exists in that version, validates `data`, and persists the result.
/// `created_by` is the acting user's ID (from `AuthContext::user_id`), or `None` for an unattributed service/automation API key.
///
/// The quota check and insert are serialized with a workspace-scoped advisory lock: without it, concurrent creates could each read a count under `max_entities` and both insert, overshooting the cap.
///
/// Runs on the RLS-scoped raw connection a request handler holds via `Authorized::conn()`, not
/// through the SeaORM entity layer: see the "which pool" rule in this repository's Loco-rebuild
/// notes (product `CLAUDE.md`).
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
    if crate::models::identity_workspaces::is_schema_pending(&mut tx, workspace_id).await? {
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

    let schema = crate::models::content_schemas::get_active_schema(
        &mut tx,
        workspace_id,
        &input.schema_name,
    )
    .await?;
    let entity_type_def = resolve_entity_type(&schema.definition, &input.entity_type)?;
    validate_data(entity_type_def, &input.data)?;

    let row: EntityRecord = sqlx::query_as(
        "INSERT INTO content_entities (workspace_id, schema_id, schema_version, entity_type, data, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, workspace_id, schema_id, schema_version, entity_type, data, \
                   created_at, updated_at, created_by, updated_by",
    )
    .bind(workspace_id)
    .bind(schema.id)
    .bind(schema.version)
    .bind(&input.entity_type)
    .bind(&input.data)
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await
    .internal()?;

    tx.commit().await.internal()?;

    Ok(row)
}

/// Runs on the RLS-scoped raw connection a request handler holds via `Authorized::conn()`, not
/// through the SeaORM entity layer: see the "which pool" rule in this repository's Loco-rebuild
/// notes (product `CLAUDE.md`).
pub async fn get(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<EntityRecord, YorishiroError> {
    sqlx::query_as(
        "SELECT id, workspace_id, schema_id, schema_version, entity_type, data, \
                created_at, updated_at, created_by, updated_by \
         FROM content_entities WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .internal()?
    .ok_or_else(|| YorishiroError::not_found(format!("entity '{id}' was not found")))
}
