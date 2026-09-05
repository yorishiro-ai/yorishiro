use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveValue, QueryOrder, QuerySelect, SqlErr};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use super::_entities::content_schemas::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::{self, MetaSchemaDefinition, VersioningDiff, validate_definition};

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    /// `id` has a `uuidv7()` column default on PostgreSQL and no default on SQLite; see `crate::db::sqlite_generated_id`.
    async fn before_save<C>(self, db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::sqlite_generated_id(db, this.id);
        // Stamped on insert as well as update, unlike the seven models that stamp only on update: those tables' `updated_at` columns carry a database default on both backends, and this one cannot.
        // SQLite refuses a non-constant default on `ADD COLUMN` for an existing table, so `content_schemas.updated_at` has `now()` on PostgreSQL and nothing on SQLite, and an insert that relied on the default would be NULL there.
        // A caller that sets it deliberately (a backfill, an import preserving original timestamps) is still not overwritten.
        if !this.updated_at.is_set() {
            this.updated_at = sea_orm::ActiveValue::Set(Some(chrono::Utc::now().into()));
        }
        Ok(this)
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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    // SQLite serializes UUIDs as binary in SeaORM queries, but the migration stores them as
    // hex strings in TEXT columns. Convert to hex for the filter so the comparison works.
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return get_active_schema_sqlite(conn, workspace_id, name).await;
    }
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

/// SQLite-format UUID: hex string without dashes, matching how SeaORM serialises
/// Uuid columns for TEXT storage.
fn uuid_hex(u: Uuid) -> String {
    u.simple().to_string()
}

/// SQLite variant of `get_active_schema`: UUIDs are stored as hex strings in TEXT columns.
/// SeaORM's `try_get::<Uuid>` always expects 16-byte binary regardless of column type, so we
/// read UUID columns as hex strings and parse them manually.
async fn get_active_schema_sqlite(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    name: &str,
) -> Result<SchemaRecord, YorishiroError> {
    use sea_orm::{DatabaseBackend, Statement};

    let sql = format!(
        "SELECT id, tenant_id, workspace_id, name, version, definition, status, \
         origin_template_id, origin_status, origin_snapshot, created_at \
         FROM content_schemas \
         WHERE workspace_id = '{}' AND name = '{}' AND status = 'active' \
         ORDER BY version DESC LIMIT 1",
        workspace_id,
        name.replace('\'', "''")
    );
    let stmt = Statement::from_string(DatabaseBackend::Sqlite, sql);
    let rows = conn.query_all_raw(stmt).await.internal()?;
    let row = rows
        .first()
        .ok_or_else(|| YorishiroError::not_found(format!("no active schema named '{name}'")))?;

    let id = Uuid::parse_str(&row.try_get::<String>("", "id").internal()?).internal()?;
    let tenant_id =
        Uuid::parse_str(&row.try_get::<String>("", "tenant_id").internal()?).internal()?;
    let workspace_id_out =
        Uuid::parse_str(&row.try_get::<String>("", "workspace_id").internal()?).internal()?;
    let schema_name: String = row.try_get("", "name").internal()?;
    let version: i32 = row.try_get("", "version").internal()?;
    let definition: serde_json::Value = row.try_get("", "definition").internal()?;
    let status: String = row.try_get("", "status").internal()?;
    let origin_template_id: Option<Uuid> = row
        .try_get::<Option<String>>("", "origin_template_id")
        .ok()
        .flatten()
        .map(|s| Uuid::parse_str(&s).internal())
        .transpose()?;
    let origin_status: String = row.try_get("", "origin_status").internal()?;
    let origin_snapshot: Option<serde_json::Value> =
        row.try_get("", "origin_snapshot").ok().flatten();
    let created_at: DateTime<Utc> = row.try_get("", "created_at").internal()?;

    Ok(SchemaRecord {
        id,
        tenant_id,
        workspace_id: workspace_id_out,
        name: schema_name,
        version,
        definition: serde_json::from_value(definition).internal()?,
        status,
        origin_template_id,
        origin_status,
        origin_snapshot: origin_snapshot
            .map(|v| serde_json::from_value(v).internal())
            .transpose()?,
        created_at,
    })
}

/// SQLite variant of `create_schema`: inserts the row directly with hex-string UUIDs so FK
/// constraints (which compare against hex-string TEXT columns) evaluate correctly.
async fn create_schema_sqlite(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    workspace_id: Uuid,
    definition: MetaSchemaDefinition,
    origin_template_id: Option<Uuid>,
    origin_snapshot: Option<MetaSchemaDefinition>,
) -> Result<(SchemaRecord, VersioningDiff), YorishiroError> {
    use sea_orm::{DatabaseBackend, Statement};

    validate_definition(&definition)?;
    tracing::debug!("create_schema_sqlite: validation OK, name={}", definition.name);

    let name = definition.name.clone();
    tracing::debug!("create_schema_sqlite: ws_hex={}, tenant_hex={}", uuid_hex(workspace_id), uuid_hex(tenant_id));

    crate::db::lock_for_update(conn, &format!("{workspace_id}:{name}"))
        .await
        .internal()?;

    // Find previous active schema version
    let sql = format!(
        "SELECT id, tenant_id, workspace_id, name, version, definition, status, \
         origin_template_id, origin_status, origin_snapshot, created_at \
         FROM content_schemas \
         WHERE workspace_id = '{}' AND name = '{}' AND status = 'active' \
         ORDER BY version DESC LIMIT 1",
        workspace_id,
        name.replace('\'', "''")
    );
    let stmt = Statement::from_string(DatabaseBackend::Sqlite, sql);
    let rows = conn.query_all_raw(stmt).await.internal()?;

    let previous: Option<SchemaRecord> = if let Some(row) = rows.first() {
        let id = Uuid::parse_str(&row.try_get::<String>("", "id").internal()?).internal()?;
        let tenant_id =
            Uuid::parse_str(&row.try_get::<String>("", "tenant_id").internal()?).internal()?;
        let workspace_id_out =
            Uuid::parse_str(&row.try_get::<String>("", "workspace_id").internal()?).internal()?;
        let schema_name: String = row.try_get("", "name").internal()?;
        let version: i32 = row.try_get("", "version").internal()?;
        let definition: serde_json::Value = row.try_get("", "definition").internal()?;
        let status: String = row.try_get("", "status").internal()?;
        let origin_template_id: Option<Uuid> = row
            .try_get::<Option<String>>("", "origin_template_id")
            .ok()
            .flatten()
            .map(|s| Uuid::parse_str(&s).internal())
            .transpose()
            .internal()?;
        let origin_status: String = row.try_get("", "origin_status").internal()?;
        let origin_snapshot: Option<serde_json::Value> =
            row.try_get("", "origin_snapshot").ok().flatten();
        let created_at: DateTime<Utc> = row.try_get("", "created_at").internal()?;
        Some(SchemaRecord {
            id,
            tenant_id,
            workspace_id: workspace_id_out,
            name: schema_name,
            version,
            definition: serde_json::from_value(definition).internal()?,
            status,
            origin_template_id,
            origin_status,
            origin_snapshot: origin_snapshot
                .map(|v| serde_json::from_value(v).internal())
                .transpose()
                .internal()?,
            created_at,
        })
    } else {
        None
    };

    // Only the first version of a name mints an origin from what the caller passed.
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
        // Archive previous active schemas
        let archive_sql = format!(
            "UPDATE content_schemas SET status = 'archived', updated_at = '{}' \
             WHERE workspace_id = '{}' AND name = '{}' AND status = 'active'",
            chrono::Utc::now().to_rfc3339(),
            workspace_id,
            name.replace('\'', "''")
        );
        conn.execute_raw(Statement::from_string(DatabaseBackend::Sqlite, archive_sql))
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

    let now = chrono::Utc::now().to_rfc3339();
    let insert_sql = format!(
        "INSERT INTO content_schemas (id, tenant_id, workspace_id, name, version, definition, status, origin_template_id, origin_status, origin_snapshot, created_at) \
         VALUES ('{}', '{}', '{}', '{}', {}, '{}', '{}', {}, '{}', '{}', '{}')",
        Uuid::now_v7(),
        tenant_id,
        workspace_id,
        name,
        next_version,
        definition_json.to_string().replace('\'', "''"),
        "active",
        origin_template_id
            .map(|u| format!("'{}'", u))
            .unwrap_or("NULL".to_string()),
        origin_status,
        origin_snapshot_json
            .map(|v| format!("'{}'", v.to_string().replace('\'', "''")))
            .unwrap_or("NULL".to_string()),
        now
    );

    conn.execute_raw(Statement::from_string(DatabaseBackend::Sqlite, insert_sql))
        .await
        .internal()?;

    // Fetch the inserted row
    let select_sql = format!(
        "SELECT id, tenant_id, workspace_id, name, version, definition, status, \
         origin_template_id, origin_status, origin_snapshot, created_at \
         FROM content_schemas \
         WHERE workspace_id = '{}' AND name = '{}' AND status = 'active' \
         ORDER BY version DESC LIMIT 1",
        workspace_id,
        name.replace('\'', "''")
    );
    let stmt = Statement::from_string(DatabaseBackend::Sqlite, select_sql);
    let rows = conn.query_all_raw(stmt).await.internal()?;
    let row = rows.first().ok_or_else(|| {
        YorishiroError::Internal(anyhow::anyhow!("schema not found after insert"))
    })?;

    let id = Uuid::parse_str(&row.try_get::<String>("", "id").internal()?).internal()?;
    let tenant_id =
        Uuid::parse_str(&row.try_get::<String>("", "tenant_id").internal()?).internal()?;
    let workspace_id_out =
        Uuid::parse_str(&row.try_get::<String>("", "workspace_id").internal()?).internal()?;
    let schema_name: String = row.try_get("", "name").internal()?;
    let version: i32 = row.try_get("", "version").internal()?;
    let definition: serde_json::Value = row.try_get("", "definition").internal()?;
    let status: String = row.try_get("", "status").internal()?;
    let origin_template_id: Option<Uuid> = row
        .try_get::<Option<String>>("", "origin_template_id")
        .ok()
        .flatten()
        .map(|s| Uuid::parse_str(&s).internal())
        .transpose()?;
    let origin_status: String = row.try_get("", "origin_status").internal()?;
    let origin_snapshot: Option<serde_json::Value> =
        row.try_get("", "origin_snapshot").ok().flatten();
    let created_at: DateTime<Utc> = row.try_get("", "created_at").internal()?;

    let record = SchemaRecord {
        id,
        tenant_id,
        workspace_id: workspace_id_out,
        name: schema_name,
        version,
        definition: serde_json::from_value(definition).internal()?,
        status,
        origin_template_id,
        origin_status,
        origin_snapshot: origin_snapshot
            .map(|v| serde_json::from_value(v).internal())
            .transpose()?,
        created_at,
    };

    // Mark workspace active
    crate::models::identity_workspaces::mark_active(conn, workspace_id, id).await?;

    Ok((record, diff))
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
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return get_by_id_sqlite(conn, workspace_id, schema_id).await;
    }
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

/// SQLite variant of `get_by_id`: uses raw SQL with hex-string UUIDs in the WHERE clause.
async fn get_by_id_sqlite(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<SchemaRecord, YorishiroError> {
    use sea_orm::{DatabaseBackend, Statement};

    let sql = format!(
        "SELECT id, tenant_id, workspace_id, name, version, definition, status, \
         origin_template_id, origin_status, origin_snapshot, created_at \
         FROM content_schemas \
         WHERE id = '{}' AND workspace_id = '{}'",
        schema_id, workspace_id
    );
    let stmt = Statement::from_string(DatabaseBackend::Sqlite, sql);
    let rows = conn.query_all_raw(stmt).await.internal()?;
    let row = rows
        .first()
        .ok_or_else(|| YorishiroError::not_found(format!("schema '{schema_id}' was not found")))?;

    let id = Uuid::parse_str(&row.try_get::<String>("", "id").internal()?).internal()?;
    let tenant_id =
        Uuid::parse_str(&row.try_get::<String>("", "tenant_id").internal()?).internal()?;
    let workspace_id_out =
        Uuid::parse_str(&row.try_get::<String>("", "workspace_id").internal()?).internal()?;
    let schema_name: String = row.try_get("", "name").internal()?;
    let version: i32 = row.try_get("", "version").internal()?;
    let definition: serde_json::Value = row.try_get("", "definition").internal()?;
    let status: String = row.try_get("", "status").internal()?;
    let origin_template_id: Option<Uuid> = row
        .try_get::<Option<String>>("", "origin_template_id")
        .ok()
        .flatten()
        .map(|s| Uuid::parse_str(&s).internal())
        .transpose()?;
    let origin_status: String = row.try_get("", "origin_status").internal()?;
    let origin_snapshot: Option<serde_json::Value> =
        row.try_get("", "origin_snapshot").ok().flatten();
    let created_at: DateTime<Utc> = row.try_get("", "created_at").internal()?;

    Ok(SchemaRecord {
        id,
        tenant_id,
        workspace_id: workspace_id_out,
        name: schema_name,
        version,
        definition: serde_json::from_value(definition).internal()?,
        status,
        origin_template_id,
        origin_status,
        origin_snapshot: origin_snapshot
            .map(|v| serde_json::from_value(v).internal())
            .transpose()?,
        created_at,
    })
}

/// A schema whose origin template has been edited since the copy was taken.
///
/// The signal only (what changed and where), with no diff and no application.
/// Whether to follow the upstream edit is the workspace's call, since applying it could invalidate entities already stored against the current definition.
///
/// Lives here rather than in `ee/` because it names `content_schemas` fields (`schema_id`, `version`) this module already owns.
/// The endpoint that produces one (`ee/`'s `GET /api/schemas/upstream-changes`) is what makes following it enterprise, not the shape of the value itself.
#[derive(Clone, Serialize)]
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
#[derive(Clone, Serialize)]
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
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return create_schema_sqlite(
            conn,
            tenant_id,
            workspace_id,
            definition,
            origin_template_id,
            origin_snapshot,
        )
        .await;
    }

    use super::_entities::content_schemas::Column;

    validate_definition(&definition)?;
    tracing::debug!("create_schema_sqlite: validation OK");

    let name = definition.name.clone();
    tracing::debug!("create_schema_sqlite: name={}, ws={}, tenant={}", name, workspace_id, tenant_id);

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
        // `update_many` is a builder call and never runs `ActiveModelBehavior::before_save`, so `updated_at` is set explicitly here or archiving a schema version would leave its timestamp stale.
        Entity::update_many()
            .col_expr(Column::Status, Expr::value("archived"))
            .col_expr(
                Column::UpdatedAt,
                Expr::value(chrono::Utc::now().fixed_offset()),
            )
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
