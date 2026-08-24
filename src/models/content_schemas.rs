use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveValue, QueryOrder, QuerySelect, SqlErr};
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
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`, so it takes anything implementing `ConnectionTrait` (a `DatabaseTransaction`, in practice).
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

/// Counts a workspace's currently *active* schemas: one row per distinct name, since `create_schema` archives the previous version before activating a new one.
/// For workspace-detail summaries, this is a more meaningful "how many schemas does this workspace define" figure than counting every archived version too.
pub async fn count_active(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<i64, YorishiroError> {
    use super::_entities::content_schemas::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Status.eq("active"))
        .count(conn)
        .await
        .internal()
        .map(|n| n as i64)
}

/// Fetches every schema version (active and archived) for the workspace, ordered by `(name, version)`, for a full-workspace data export.
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
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`, so it takes anything implementing `ConnectionTrait` (a `DatabaseTransaction`, in practice).
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

/// A schema whose origin template has been edited since the copy was taken.
///
/// The signal only (what changed and where), with no diff and no application.
/// Whether to follow the upstream edit is the workspace's call, since applying it could invalidate entities already stored against the current definition.
///
/// Lives here rather than in `ee/` because it names `content_schemas` fields (`schema_id`, `version`) this module already owns.
/// The endpoint that produces one (`ee/`'s `GET /api/schemas/upstream-changes`) is what makes following it enterprise, not the shape of the value itself.
#[derive(Debug, Clone, Serialize)]
pub struct UpstreamChange {
    pub schema_id: Uuid,
    pub schema_name: String,
    /// The version of the schema currently in use here.
    pub version: i32,
    pub template_id: Uuid,
    pub template_name: String,
    /// When the template was last edited.
    pub changed_at: DateTime<Utc>,
}

/// A row in a schema listing.
/// A lightweight summary that omits the `definition` body, used as the entry point for MCP clients (LLMs) to discover what schemas exist for a workspace.
#[derive(Debug, Clone, Serialize)]
pub struct SchemaSummary {
    pub id: Uuid,
    pub name: String,
    pub version: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

impl From<Model> for SchemaSummary {
    fn from(model: Model) -> Self {
        Self {
            id: model.id,
            name: model.name,
            version: model.version,
            status: model.status,
            created_at: model.created_at.into(),
        }
    }
}

/// Lists all of a workspace's schemas (every version, including archived) ordered by name and version.
pub async fn list(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    page: super::pagination::ListParams,
) -> Result<Vec<SchemaSummary>, YorishiroError> {
    use super::_entities::content_schemas::Column;

    let rows = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(Column::Name)
        .order_by_asc(Column::Version)
        .limit(page.limit() as u64)
        .offset(page.offset() as u64)
        .all(conn)
        .await
        .internal()?;

    Ok(rows.into_iter().map(SchemaSummary::from).collect())
}

pub async fn create_schema(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    workspace_id: Uuid,
    definition: MetaSchemaDefinition,
    origin_template_id: Option<Uuid>,
    origin_snapshot: Option<MetaSchemaDefinition>,
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

    // Only the first version of a name mints an origin from what the caller passed.
    // Every later one inherits the origin its predecessor already had, unless the caller states a new one: editing a schema does not un-link it from the template it was copied from, and treating an omitted origin as "detach" would silently break that link on every version after the first.
    let (origin_template_id, origin_snapshot) = match &previous {
        Some(previous) if origin_template_id.is_none() => (
            previous.origin_template_id,
            previous.origin_snapshot.clone(),
        ),
        _ => (origin_template_id, origin_snapshot),
    };

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
    let origin_snapshot_json = origin_snapshot
        .map(|snapshot| serde_json::to_value(&snapshot))
        .transpose()
        .internal()?;
    let origin_status = if origin_template_id.is_some() {
        ORIGIN_STATUS_LINKED
    } else {
        ORIGIN_STATUS_DETACHED
    };

    let active = ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        workspace_id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set(name.clone()),
        version: ActiveValue::Set(next_version),
        definition: ActiveValue::Set(definition_json),
        status: ActiveValue::Set("active".to_string()),
        origin_template_id: ActiveValue::Set(origin_template_id),
        origin_status: ActiveValue::Set(origin_status.to_string()),
        origin_snapshot: ActiveValue::Set(origin_snapshot_json),
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

    // Inside the transaction: a workspace must not be left active by a schema insert that then rolls back.
    // Unconditional and idempotent: every version after the first finds it active already, and checking first would only add a round trip.
    crate::models::identity_workspaces::mark_active(conn, workspace_id, row.id).await?;

    Ok((row.try_into()?, diff))
}
