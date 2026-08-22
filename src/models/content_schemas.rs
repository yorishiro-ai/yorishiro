use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveValue, QueryOrder, SqlErr};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use super::_entities::content_schemas::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::{self, MetaSchemaDefinition, VersioningDiff, validate_definition};

pub type ContentSchemas = Entity;

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

/// Following a template that still exists.
pub const ORIGIN_STATUS_LINKED: &str = "linked";

/// Not following anything: written by hand, or following a template that has since been deleted.
pub const ORIGIN_STATUS_DETACHED: &str = "detached";

/// Represents a row in the `content_schemas` table.
/// `definition` is JSONB in the DB, but the application layer always treats it as a parsed `MetaSchemaDefinition`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub version: i32,
    pub definition: MetaSchemaDefinition,
    pub status: String,
    pub origin_template_id: Option<Uuid>,
    pub origin_status: String,
    pub origin_snapshot: Option<MetaSchemaDefinition>,
    pub created_at: DateTime<Utc>,
}

impl TryFrom<Model> for SchemaRecord {
    type Error = YorishiroError;

    fn try_from(row: Model) -> Result<Self, Self::Error> {
        Ok(SchemaRecord {
            id: row.id,
            tenant_id: row.tenant_id,
            workspace_id: row.workspace_id,
            name: row.name,
            version: row.version,
            definition: serde_json::from_value(row.definition).internal()?,
            status: row.status,
            origin_template_id: row.origin_template_id,
            origin_status: row.origin_status,
            origin_snapshot: row
                .origin_snapshot
                .map(serde_json::from_value)
                .transpose()
                .internal()?,
            created_at: row.created_at.into(),
        })
    }
}

/// Fetches the currently active schema (the latest version with status='active') for the given workspace and name.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`, so it
/// takes anything implementing `ConnectionTrait` (a `DatabaseTransaction`, in practice).
pub async fn get_active_schema(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    name: &str,
) -> Result<SchemaRecord, YorishiroError> {
    use super::_entities::content_schemas::Column;

    let row = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Name.eq(name))
        .filter(Column::Status.eq("active"))
        .order_by_desc(Column::Version)
        .one(conn)
        .await
        .internal()?;

    match row {
        Some(row) => row.try_into(),
        None => Err(YorishiroError::not_found(format!(
            "no active schema named '{name}'"
        ))),
    }
}

/// Fetches every schema version (active and archived) for the workspace, ordered by
/// `(name, version)`, for a full-workspace data export.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn export_all(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<SchemaRecord>, YorishiroError> {
    use super::_entities::content_schemas::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(Column::Name)
        .order_by_asc(Column::Version)
        .all(conn)
        .await
        .internal()?
        .into_iter()
        .map(SchemaRecord::try_from)
        .collect()
}

/// Fetches a specific schema version by id (used to resolve the version an entity references).
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`, so it
/// takes anything implementing `ConnectionTrait` (a `DatabaseTransaction`, in practice).
pub async fn get_by_id(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<SchemaRecord, YorishiroError> {
    use super::_entities::content_schemas::Column;

    let row = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(schema_id))
        .one(conn)
        .await
        .internal()?;

    match row {
        Some(row) => row.try_into(),
        None => Err(YorishiroError::not_found(format!(
            "schema '{schema_id}' was not found"
        ))),
    }
}

/// Registers a new schema definition, after validating it with `validate_definition`.
/// If no schema of this name exists yet, creates version 1 as active; otherwise computes a
/// `versioning::diff` against the current active version, archives it, and always inserts the
/// new definition as the next version (reporting whether the diff is breaking).
///
/// The template-origin chain (`origin_template_id` linkage, upstream-change detection,
/// three-way merge base) is not ported yet: this always inserts with no origin
/// (`origin_status = detached`), matching a schema written by hand. Templates are a later slice.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`. That
/// transaction is also this function's lock/read/archive/insert scope: it does not open a
/// nested transaction of its own, since the request transaction already is the unit of work
/// (the caller commits it after this returns, via `Authorized::commit()`).
pub async fn create_schema(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    workspace_id: Uuid,
    definition: MetaSchemaDefinition,
) -> Result<(SchemaRecord, VersioningDiff), YorishiroError> {
    use super::_entities::content_schemas::Column;

    validate_definition(&definition)?;

    let name = definition.name.clone();

    crate::db::lock_for_update(conn, &format!("{workspace_id}:{name}"))
        .await
        .internal()?;

    let previous = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Name.eq(&name))
        .filter(Column::Status.eq("active"))
        .order_by_desc(Column::Version)
        .one(conn)
        .await
        .internal()?
        .map(SchemaRecord::try_from)
        .transpose()?;

    let (next_version, diff) = match &previous {
        Some(previous) => {
            let diff = metaschema::diff(&previous.definition, &definition);
            (previous.version + 1, diff)
        }
        None => (
            1,
            VersioningDiff {
                is_breaking: false,
                reasons: Vec::new(),
            },
        ),
    };

    if previous.is_some() {
        Entity::update_many()
            .col_expr(Column::Status, Expr::value("archived"))
            .filter(Column::WorkspaceId.eq(workspace_id))
            .filter(Column::Name.eq(&name))
            .filter(Column::Status.eq("active"))
            .exec(conn)
            .await
            .internal()?;
    }

    let definition_json = serde_json::to_value(&definition).internal()?;

    let active = ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        workspace_id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set(name.clone()),
        version: ActiveValue::Set(next_version),
        definition: ActiveValue::Set(definition_json),
        status: ActiveValue::Set("active".to_string()),
        origin_status: ActiveValue::Set(ORIGIN_STATUS_DETACHED.to_string()),
        ..Default::default()
    };
    let row = active.insert(conn).await.map_err(|err| {
        if matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
            YorishiroError::Conflict {
                message: format!(
                    "schema '{name}' version {next_version} already exists (concurrent create?)"
                ),
            }
        } else {
            YorishiroError::Internal(err.into())
        }
    })?;

    // Inside the transaction: a workspace must not be left active by a schema insert that then
    // rolls back. Unconditional and idempotent: every version after the first finds it active
    // already, and checking first would only add a round trip.
    crate::models::identity_workspaces::mark_active(conn, workspace_id, row.id).await?;

    Ok((row.try_into()?, diff))
}
