use chrono::{DateTime, Utc};
use sea_query::{Alias, Asterisk, Expr, Func, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Connection, PgConnection};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::{self, MetaSchemaDefinition, VersioningDiff, validate_definition};

/// Represents a row in the `schemas` table.
/// `definition` is JSONB in the DB, but the application layer always treats it as a parsed `MetaSchemaDefinition`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SchemaRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    /// The workspace that owns this schema.
    /// Schemas are per workspace: applying a template gives a workspace its own copy, so a sibling workspace's edits do not reach it.
    ///
    /// `default` on deserialize because this type is also the JSONL export record, and an export taken before schemas became workspace-scoped carries no such field.
    /// Import assigns the destination workspace regardless (it remaps every id it reads), so the value in the file is never the one that lands.
    #[serde(default)]
    pub workspace_id: Uuid,
    pub name: String,
    pub version: i32,
    pub definition: MetaSchemaDefinition,
    pub status: String,
    /// The template this schema was created from, when it was created from one.
    /// `None` for a schema written by hand, and `None` again once that template is deleted.
    ///
    /// `default` on deserialize for the same reason as `workspace_id`: this type doubles as the JSONL export record, and an export taken before the column existed carries no such field.
    #[serde(default)]
    pub origin_template_id: Option<Uuid>,
    /// `linked` while the origin is still there to follow, `detached` otherwise, including for every schema that never had one.
    #[serde(default = "default_origin_status")]
    pub origin_status: String,
    /// The template's definition as it stood when this copy was taken: the common ancestor a three-way merge compares against.
    ///
    /// `None` for a schema with no origin, and for one copied before this was recorded: what the template said then is not recoverable, and a fabricated base is worse than none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_snapshot: Option<MetaSchemaDefinition>,
    pub created_at: DateTime<Utc>,
}

fn default_origin_status() -> String {
    ORIGIN_STATUS_DETACHED.to_string()
}

/// Following a template that still exists.
pub const ORIGIN_STATUS_LINKED: &str = "linked";

/// Not following anything: written by hand, or following a template that has since been deleted.
/// The two are told apart by whether `origin_template_id` was ever set, which is what the notification path needs to know.
pub const ORIGIN_STATUS_DETACHED: &str = "detached";

/// A schema whose origin template has been edited since the copy was taken.
///
/// The signal only (what changed and where), with no diff and no application.
/// Whether to follow the upstream edit is the workspace's call, since applying it could invalidate entities already stored against the current definition.
#[derive(Debug, Clone, Serialize, ToSchema)]
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
/// A lightweight summary that omits the `definition` body, used as the entry point for MCP clients (LLMs) to discover what schemas exist for a tenant.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct SchemaSummary {
    pub id: Uuid,
    pub name: String,
    pub version: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Iden)]
enum Schemas {
    Table,
    Id,
    TenantId,
    WorkspaceId,
    Name,
    Version,
    Definition,
    Status,
    OriginTemplateId,
    OriginStatus,
    OriginSnapshot,
    CreatedAt,
}

#[derive(sqlx::FromRow)]
struct SchemaRow {
    id: Uuid,
    tenant_id: Uuid,
    workspace_id: Uuid,
    name: String,
    version: i32,
    definition: Value,
    status: String,
    origin_template_id: Option<Uuid>,
    origin_status: String,
    origin_snapshot: Option<Value>,
    created_at: DateTime<Utc>,
}

impl SchemaRow {
    fn into_record(self) -> Result<SchemaRecord, YorishiroError> {
        let definition = serde_json::from_value(self.definition).internal()?;
        Ok(SchemaRecord {
            id: self.id,
            tenant_id: self.tenant_id,
            workspace_id: self.workspace_id,
            name: self.name,
            version: self.version,
            definition,
            status: self.status,
            origin_template_id: self.origin_template_id,
            origin_status: self.origin_status,
            origin_snapshot: self
                .origin_snapshot
                .map(serde_json::from_value)
                .transpose()
                .internal()?,
            created_at: self.created_at,
        })
    }
}

/// Lists all of a tenant's schemas (every version, including archived) ordered by name and version.
pub async fn list<C>(conn: &mut C, workspace_id: Uuid) -> Result<Vec<SchemaSummary>, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    SchemaSummary: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let (sql, values) = Query::select()
        .columns([
            Schemas::Id,
            Schemas::Name,
            Schemas::Version,
            Schemas::Status,
            Schemas::CreatedAt,
        ])
        .from((Alias::new("content"), Schemas::Table))
        .and_where(Expr::col(Schemas::WorkspaceId).eq(workspace_id))
        .order_by(Schemas::Name, Order::Asc)
        .order_by(Schemas::Version, Order::Asc)
        .build_sqlx(C::builder());

    sqlx::query_as_with::<_, SchemaSummary, _>(&sql, values)
        .fetch_all(&mut *conn)
        .await
        .internal()
}

/// Counts a tenant's currently *active* schemas: one row per distinct schema name, since `create_schema` archives the previous version before activating a new one.
/// For tenant-detail summaries, this is a more meaningful "how many schemas does this tenant define" figure than counting every archived version too.
pub async fn count_active<C>(conn: &mut C, workspace_id: Uuid) -> Result<i64, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    (i64,): for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let (sql, values) = Query::select()
        .expr(Func::count(Expr::col(Asterisk)))
        .from((Alias::new("content"), Schemas::Table))
        .and_where(Expr::col(Schemas::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Schemas::Status).eq("active"))
        .build_sqlx(C::builder());
    let (count,): (i64,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .internal()?;
    Ok(count)
}

fn schema_columns() -> [Schemas; 11] {
    [
        Schemas::Id,
        Schemas::TenantId,
        Schemas::WorkspaceId,
        Schemas::Name,
        Schemas::Version,
        Schemas::Definition,
        Schemas::Status,
        Schemas::OriginTemplateId,
        Schemas::OriginStatus,
        Schemas::OriginSnapshot,
        Schemas::CreatedAt,
    ]
}

/// Fetches the currently active schema (the latest version with status='active') for the given tenant and name.
// `SchemaRow` stays private: its `FromRow` impl is generic over any `Row`, so no external caller ever needs to name it to satisfy this bound.
#[allow(private_bounds)]
pub async fn get_active_schema<C>(
    conn: &mut C,
    workspace_id: Uuid,
    name: &str,
) -> Result<SchemaRecord, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    SchemaRow: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let (sql, values) = Query::select()
        .columns(schema_columns())
        .from((Alias::new("content"), Schemas::Table))
        .and_where(Expr::col(Schemas::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Schemas::Name).eq(name))
        .and_where(Expr::col(Schemas::Status).eq("active"))
        .order_by(Schemas::Version, Order::Desc)
        .limit(1)
        .build_sqlx(C::builder());
    let row: Option<SchemaRow> = sqlx::query_as_with(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .internal()?;

    match row {
        Some(row) => row.into_record(),
        None => Err(YorishiroError::not_found(format!(
            "no active schema named '{name}'"
        ))),
    }
}

/// Fetches a specific schema version by id (used to resolve the version an entity references).
#[allow(private_bounds)]
pub async fn get_by_id<C>(
    conn: &mut C,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<SchemaRecord, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    SchemaRow: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let (sql, values) = Query::select()
        .columns(schema_columns())
        .from((Alias::new("content"), Schemas::Table))
        .and_where(Expr::col(Schemas::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Schemas::Id).eq(schema_id))
        .build_sqlx(C::builder());
    let row: Option<SchemaRow> = sqlx::query_as_with(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .internal()?;

    match row {
        Some(row) => row.into_record(),
        None => Err(YorishiroError::not_found(format!(
            "schema '{schema_id}' was not found"
        ))),
    }
}

/// Fetches every schema version for the tenant (including archived), with no pagination limit and the full `definition` body, for a full-tenant export.
#[allow(private_bounds)]
pub async fn export_all<C>(
    conn: &mut C,
    workspace_id: Uuid,
) -> Result<Vec<SchemaRecord>, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    SchemaRow: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let (sql, values) = Query::select()
        .columns(schema_columns())
        .from((Alias::new("content"), Schemas::Table))
        .and_where(Expr::col(Schemas::WorkspaceId).eq(workspace_id))
        .order_by(Schemas::Name, Order::Asc)
        .order_by(Schemas::Version, Order::Asc)
        .build_sqlx(C::builder());
    let rows: Vec<SchemaRow> = sqlx::query_as_with(&sql, values)
        .fetch_all(&mut *conn)
        .await
        .internal()?;

    rows.into_iter().map(SchemaRow::into_record).collect()
}

/// Registers a new schema definition, after validating it with `validate_definition`.
/// If no schema of this name exists yet, creates version 1 as active; otherwise computes a a `versioning::diff`, archives the previous active version, and always inserts the new definition as the next version (reporting whether the diff is breaking).
///
/// Concurrent creates for the same (workspace_id, name) are serialized with an advisory lock:
/// without it, reading the active version and then archiving-it-plus-inserting the new one would race, letting concurrent calls fail on the UNIQUE(workspace_id, name, version) constraint or archive a version another call just committed as active.
pub async fn create_schema(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    workspace_id: Uuid,
    definition: MetaSchemaDefinition,
) -> Result<(SchemaRecord, VersioningDiff), YorishiroError> {
    create_schema_from(conn, tenant_id, workspace_id, definition, None).await
}

/// As [`create_schema`], recording which library template the definition came from.
///
/// Only a library template is passed: a built-in has no row to point at, and a definition posted inline came from nowhere.
/// Both leave the origin unset, which is what `detached` means for them: never linked, rather than linked and since orphaned.
///
/// The merge base is taken to be the definition itself, which holds when the definition *is* the template's.
/// A caller writing something else against a template (a copy with edits already applied) must state the template's own definition with [`create_schema_with_base`], or the base will claim the edits came from upstream and a later merge will remove them as an upstream deletion.
pub async fn create_schema_from(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    workspace_id: Uuid,
    definition: MetaSchemaDefinition,
    origin_template_id: Option<Uuid>,
) -> Result<(SchemaRecord, VersioningDiff), YorishiroError> {
    // A fresh copy is its own merge base: nothing has diverged from it yet.
    create_schema_with_base(
        conn,
        tenant_id,
        workspace_id,
        definition,
        origin_template_id,
        None,
    )
    .await
}

/// As [`create_schema_from`], stating the merge base rather than deriving it.
///
/// A copy's base is the copy itself, which is what [`create_schema_from`] records.
/// A *merge* result's base is not: it is what upstream said at the moment of the merge.
/// Recording the merged definition instead would leave the next merge reading this workspace's own edits as already present upstream, and dropping them as "unchanged here": the exact failure the three-way base exists to prevent.
///
/// `origin_snapshot` is only consulted when there is an origin at all; without one there is nothing for a base to be an ancestor of.
pub async fn create_schema_with_base(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    workspace_id: Uuid,
    definition: MetaSchemaDefinition,
    origin_template_id: Option<Uuid>,
    origin_snapshot: Option<MetaSchemaDefinition>,
) -> Result<(SchemaRecord, VersioningDiff), YorishiroError> {
    validate_definition(&definition)?;

    let name = definition.name.clone();

    let mut tx = conn.begin().await.internal()?;

    // `pg_advisory_xact_lock(...)` is a lock-acquisition function call, not a table operation:
    // no SELECT/INSERT/UPDATE/DELETE form exists for sea-query to build, same category as the session commands in `db.rs`/`auth.rs`.
    crate::db::lock_for_update(&mut tx, &format!("{workspace_id}:{name}"))
        .await
        .internal()?;

    let (sql, values) = Query::select()
        .columns(schema_columns())
        .from((Alias::new("content"), Schemas::Table))
        .and_where(Expr::col(Schemas::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Schemas::Name).eq(&name))
        .and_where(Expr::col(Schemas::Status).eq("active"))
        .order_by(Schemas::Version, Order::Desc)
        .limit(1)
        .build_sqlx(PostgresQueryBuilder);
    let previous_row: Option<SchemaRow> = sqlx::query_as_with(&sql, values)
        .fetch_optional(&mut *tx)
        .await
        .internal()?;

    // Only the first version of a name mints a base from its own definition.
    // Every later one inherits the base it already had, unless the caller states a new one: editing a schema does not change what the template said when it was copied, and resetting the base to the edit would record this workspace's own fields as upstream's, after which the next merge reads them as "unchanged here" and follows an upstream removal by deleting them.
    let mut inherited_snapshot = None;

    let (next_version, diff) = match previous_row {
        Some(row) => {
            let previous = row.into_record()?;
            let diff = metaschema::diff(&previous.definition, &definition);
            inherited_snapshot = previous.origin_snapshot;
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

    let (sql, values) = Query::update()
        .table((Alias::new("content"), Schemas::Table))
        .values([(Schemas::Status, "archived".into())])
        .and_where(Expr::col(Schemas::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Schemas::Name).eq(&name))
        .and_where(Expr::col(Schemas::Status).eq("active"))
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values)
        .execute(&mut *tx)
        .await
        .internal()?;

    let definition_json = serde_json::to_value(&definition).internal()?;

    let (sql, values) = Query::insert()
        .into_table((Alias::new("content"), Schemas::Table))
        .columns([
            Schemas::TenantId,
            Schemas::WorkspaceId,
            Schemas::Name,
            Schemas::Version,
            Schemas::Definition,
            Schemas::Status,
            Schemas::OriginTemplateId,
            Schemas::OriginStatus,
            Schemas::OriginSnapshot,
        ])
        .values_panic([
            tenant_id.into(),
            workspace_id.into(),
            name.clone().into(),
            next_version.into(),
            definition_json.clone().into(),
            "active".into(),
            origin_template_id.into(),
            // A schema with no origin is detached, not linked-to-nothing.
            if origin_template_id.is_some() {
                ORIGIN_STATUS_LINKED.into()
            } else {
                ORIGIN_STATUS_DETACHED.into()
            },
            // The merge base, in order of authority: what the caller states (a merge knows the base moved to upstream), then what the previous version carried (an edit does not move it), then this definition itself (the first copy is its own ancestor).
            // Only meaningful with an origin: without one there is nothing to be an ancestor of.
            match (
                origin_template_id,
                origin_snapshot.as_ref().or(inherited_snapshot.as_ref()),
            ) {
                (None, _) => sea_query::Value::Json(None).into(),
                (Some(_), Some(base)) => serde_json::to_value(base).internal()?.into(),
                (Some(_), None) => definition_json.into(),
            },
        ])
        .returning(Query::returning().columns(schema_columns()))
        .build_sqlx(PostgresQueryBuilder);
    let row: SchemaRow = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            if err
                .as_database_error()
                .is_some_and(|db_err| db_err.is_unique_violation())
            {
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
    crate::models::tenancy::mark_active(&mut *tx, workspace_id, row.id).await?;

    tx.commit().await.internal()?;

    Ok((row.into_record()?, diff))
}

#[cfg(test)]
#[path = "../../../tests/models/schemas/mod.rs"]
mod tests;
