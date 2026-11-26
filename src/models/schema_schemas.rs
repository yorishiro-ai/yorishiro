//! Schema model: versioned schemas per workspace, with the version lock, archiving and deleting of versions.

use crate::error::OptionExt;
use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::{Alias, Expr, Func, Query};
use sea_orm::{ActiveValue, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
pub use crate::models::_entities::schema_schemas::{ActiveModel, Entity, Model};
use crate::models::db_enum::db_enum;

pub mod metaschema;

pub(crate) async fn workspace_for_actor(
    db: &impl ConnectionTrait,
    actor: &crate::models::access_keys::AuthContext,
    id: Uuid,
) -> Result<Uuid, YorishiroError> {
    let record = Entity::find_by_id(id)
        .one(db)
        .await
        .internal()?
        .or_not_found("schema was not found")?;
    crate::models::iam::access::check_workspace_target(db, actor, record.workspace_id).await
}

use metaschema::{MetaSchemaDefinition, VersioningDiff, validate_definition};

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::generated_id(this.id);
        this.created_at = crate::db::stamped_created_at(insert, this.created_at);
        this.updated_at = crate::db::stamped_updated_at_optional(this.updated_at);
        Ok(this)
    }
}

db_enum! {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum SchemaStatus {
        Active = "active",
        Archived = "archived",
    }
}

db_enum! {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum SchemaOriginStatus {
        Linked = "linked",
        Detached = "detached",
    }
}

/// Serializes every producer of a schema version for one workspace and name.
///
/// Fork heads use this same lock as ordinary schema creation, so both paths
/// observe one version sequence even when they run concurrently.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn lock_version(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    name: &str,
) -> Result<(), YorishiroError> {
    crate::db::lock_for_update(conn, &format!("{workspace_id}:{name}"))
        .await
        .internal()
}

/// Retires one version without deleting entity, migration or fork history.
///
/// # Errors
/// Returns an error when the version does not belong to the selected workspace or persistence fails.
pub(crate) async fn archive(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<SchemaRecord, YorishiroError> {
    use crate::models::_entities::schema_schemas::Column;

    let schema = get_by_id(conn, workspace_id, schema_id).await?;
    lock_version(conn, workspace_id, &schema.name).await?;
    Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(SchemaStatus::Archived.as_db_str()),
        )
        .col_expr(Column::UpdatedAt, Expr::value(crate::data::time::now()))
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(schema_id))
        .filter(Column::Status.eq(SchemaStatus::Active.as_db_str()))
        .exec(conn)
        .await
        .internal()?;
    get_by_id(conn, workspace_id, schema_id).await
}

/// Deletes one archived schema version that nothing references.
///
/// # Errors
/// Returns an error when the version is still active, when entities, snapshots, proposals, forks or workspaces still point at it, or when persistence fails.
pub(crate) async fn delete(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<(), YorishiroError> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let schema = get_by_id(conn, workspace_id, schema_id).await?;
    lock_version(conn, workspace_id, &schema.name).await?;
    // The row lock also fences reference writers that rely on the foreign key rather than the version lock.
    let row = Entity::find_by_id(schema_id)
        .filter(crate::models::_entities::schema_schemas::Column::WorkspaceId.eq(workspace_id))
        .lock_exclusive()
        .one(conn)
        .await
        .internal()?
        .or_not_found(format!("schema '{schema_id}' was not found"))?;
    let schema = SchemaRecord::try_from(row)?;
    if schema.status != SchemaStatus::Archived {
        return Err(YorishiroError::Conflict {
            message: format!(
                "schema '{}' version {} is still active; archive it before deleting",
                schema.name, schema.version
            ),
        });
    }
    // References are retained history, so deletion refuses and names them instead of cascading.
    let blockers = deletion_blockers(conn, schema_id).await?;
    if !blockers.is_empty() {
        return Err(YorishiroError::Conflict {
            message: format!(
                "schema '{}' version {} is still referenced by: {}",
                schema.name,
                schema.version,
                blockers.join(", ")
            ),
        });
    }
    let deleted = Entity::delete_by_id(schema_id)
        .exec(conn)
        .await
        .internal()?;
    if deleted.rows_affected == 0 {
        return Err(YorishiroError::not_found(format!(
            "schema '{schema_id}' was not found"
        )));
    }
    Ok(())
}

async fn deletion_blockers(
    conn: &impl ConnectionTrait,
    schema_id: Uuid,
) -> Result<Vec<&'static str>, YorishiroError> {
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};

    let mut blockers: Vec<&str> = Vec::new();
    let entities = crate::models::_entities::entity_entities::Entity::find()
        .filter(crate::models::_entities::entity_entities::Column::SchemaId.eq(schema_id))
        .count(conn)
        .await
        .internal()?;
    if entities > 0 {
        blockers.push("entities");
    }
    let snapshots = crate::models::_entities::entity_snapshots::Entity::find()
        .filter(crate::models::_entities::entity_snapshots::Column::SchemaId.eq(schema_id))
        .count(conn)
        .await
        .internal()?;
    if snapshots > 0 {
        blockers.push("entity snapshots");
    }
    let proposals = crate::models::_entities::inference_proposals::Entity::find()
        .filter(crate::models::_entities::inference_proposals::Column::SchemaId.eq(schema_id))
        .count(conn)
        .await
        .internal()?;
    if proposals > 0 {
        blockers.push("inference proposals");
    }
    let workspaces = crate::models::_entities::workspace_workspaces::Entity::find()
        .filter(
            crate::models::_entities::workspace_workspaces::Column::SchemaId.eq(Some(schema_id)),
        )
        .count(conn)
        .await
        .internal()?;
    if workspaces > 0 {
        blockers.push("workspaces");
    }
    let forks = crate::models::_entities::workspace_schema_forks::Entity::find()
        .filter(
            sea_orm::Condition::any()
                .add(
                    crate::models::_entities::workspace_schema_forks::Column::SourceSchemaId
                        .eq(schema_id),
                )
                .add(
                    crate::models::_entities::workspace_schema_forks::Column::ForkSchemaId
                        .eq(schema_id),
                ),
        )
        .count(conn)
        .await
        .internal()?;
    let fork_heads = crate::models::_entities::workspace_schema_fork_heads::Entity::find()
        .filter(
            crate::models::_entities::workspace_schema_fork_heads::Column::SchemaId.eq(schema_id),
        )
        .count(conn)
        .await
        .internal()?;
    let (forks, fork_heads) = if crate::db::is_postgres(conn) {
        // Incoming forks can live outside the selected workspace, so ordinary RLS-scoped counts miss them.
        let counts = Query::select()
            .columns([Alias::new("forks"), Alias::new("fork_heads")])
            .from_function(
                Func::cust(Alias::new("schema_fork_reference_counts")).arg(Expr::val(schema_id)),
                Alias::new("counts"),
            )
            .to_owned();
        let row = conn
            .query_one_raw(conn.get_database_backend().build(&counts))
            .await
            .internal()?
            .ok_or_else(|| {
                YorishiroError::not_found(format!("schema '{schema_id}' was not found"))
            })?;
        (
            u64::try_from(row.try_get::<i64>("", "forks").internal()?).internal()?,
            u64::try_from(row.try_get::<i64>("", "fork_heads").internal()?).internal()?,
        )
    } else {
        (forks, fork_heads)
    };
    if forks > 0 {
        blockers.push("schema forks");
    }
    if fork_heads > 0 {
        blockers.push("schema fork heads");
    }
    Ok(blockers)
}

/// One version of a workspace's schema.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SchemaRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub version: i32,
    #[schemars(with = "serde_json::Value")]
    pub definition: MetaSchemaDefinition,
    pub status: SchemaStatus,
    pub origin_template_id: Option<Uuid>,
    pub origin_status: SchemaOriginStatus,
    #[schemars(with = "Option<serde_json::Value>")]
    pub origin_snapshot: Option<MetaSchemaDefinition>,
    pub origin_updated_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl TryFrom<Model> for SchemaRecord {
    type Error = YorishiroError;

    fn try_from(row: Model) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            tenant_id: row.tenant_id,
            workspace_id: row.workspace_id,
            name: row.name,
            version: row.version,
            definition: serde_json::from_value(row.definition).internal()?,
            status: SchemaStatus::from_db_str(&row.status).ok_or_else(|| {
                YorishiroError::Internal(format!("unknown schema status: {}", row.status).into())
            })?,
            origin_template_id: row.origin_template_id,
            origin_status: SchemaOriginStatus::from_db_str(&row.origin_status).ok_or_else(
                || {
                    YorishiroError::Internal(
                        format!("unknown schema origin status: {}", row.origin_status).into(),
                    )
                },
            )?,
            origin_snapshot: row
                .origin_snapshot
                .map(serde_json::from_value)
                .transpose()
                .internal()?,
            origin_updated_at: row.origin_updated_at.map(Into::into),
            created_at: row.created_at.into(),
        })
    }
}

/// Fetches the active schema (the latest active version) for the given workspace and name.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn get_active_schema(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    name: &str,
) -> Result<SchemaRecord, YorishiroError> {
    use crate::models::_entities::schema_schemas::Column;

    let row = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Name.eq(name))
        .filter(Column::Status.eq(SchemaStatus::Active.as_db_str()))
        .order_by_desc(Column::Version)
        .one(conn)
        .await
        .internal()?;

    row.or_not_found(format!("no active schema named '{name}'"))?
        .try_into()
}

/// Counts a workspace's active schemas, which is one per name.
pub(crate) async fn count_active(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<i64, YorishiroError> {
    use crate::models::_entities::schema_schemas::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Status.eq(SchemaStatus::Active.as_db_str()))
        .count(conn)
        .await
        .internal()
        .and_then(|count| i64::try_from(count).internal())
}

/// Fetches a specific schema version by id.
pub(crate) async fn get_by_id(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<SchemaRecord, YorishiroError> {
    use crate::models::_entities::schema_schemas::Column;

    let row = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(schema_id))
        .one(conn)
        .await
        .internal()?;

    row.or_not_found(format!("schema '{schema_id}' was not found"))?
        .try_into()
}

/// A schema whose origin template has been edited since the copy was taken.
///
/// The signal only: whether to follow the upstream edit is the workspace's call, since applying it could invalidate stored entities.
#[derive(Clone, Serialize, schemars::JsonSchema)]
pub struct UpstreamChange {
    pub workspace_id: Uuid,
    pub schema_id: Uuid,
    pub schema_name: String,
    /// The version of the schema currently in use here.
    pub version: i32,
    pub template_id: Uuid,
    pub(crate) template_name: String,
    /// When the template was last edited.
    pub(crate) changed_at: DateTime<Utc>,
    /// Whether a push notification has been sent for this schema's upstream change.
    pub pending_notification: bool,
    pub summary: MergeDiffSummary,
}

/// Counts the decisions in a computed origin merge plan.
#[derive(Clone, Default, Serialize, schemars::JsonSchema)]
pub struct MergeDiffSummary {
    pub total_fields: usize,
    pub auto_add: usize,
    pub(crate) auto_update: usize,
    pub keep_local: usize,
    pub conflict: usize,
    pub has_conflicts: bool,
}

/// A schema in a listing.
/// A lightweight summary that omits the `definition` body.
#[derive(Clone, Serialize, schemars::JsonSchema)]
pub(crate) struct SchemaSummary {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub version: i32,
    pub status: SchemaStatus,
    pub created_at: DateTime<Utc>,
}

impl From<Model> for SchemaSummary {
    fn from(model: Model) -> Self {
        Self {
            id: model.id,
            workspace_id: model.workspace_id,
            name: model.name,
            version: model.version,
            status: SchemaStatus::from_db_str(&model.status).unwrap_or(SchemaStatus::Archived),
            created_at: model.created_at.into(),
        }
    }
}

/// Lists all of a workspace's schemas (every version, including archived) ordered by name and version.
pub(crate) async fn list(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<SchemaSummary>, YorishiroError> {
    use crate::models::_entities::schema_schemas::Column;

    let rows = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(Column::Name)
        .order_by_asc(Column::Version)
        .order_by_asc(Column::Id)
        .limit(page.limit())
        .offset(page.offset())
        .all(conn)
        .await
        .internal()?;

    Ok(rows.into_iter().map(SchemaSummary::from).collect())
}

pub(crate) async fn list_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::access_keys::AuthContext,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<SchemaSummary>, YorishiroError> {
    use crate::models::_entities::schema_schemas::Column;
    use crate::models::iam::{access::read_workspaces, policy::Action};
    let workspaces = read_workspaces(&ctx.db, actor, Action::ListSchemas).await?;
    let txn = crate::db::begin_workspace_list(ctx, actor.tenant_id, &workspaces).await?;
    if let [workspace] = workspaces.as_slice() {
        let records = list(&txn, *workspace, page).await?;
        txn.commit().await.internal()?;
        return Ok(records);
    }
    let rows = Entity::find()
        .filter(Column::TenantId.eq(actor.tenant_id))
        .filter(Column::WorkspaceId.is_in(workspaces))
        .order_by_asc(Column::Name)
        .order_by_asc(Column::Version)
        .order_by_asc(Column::Id)
        .limit(page.limit())
        .offset(page.offset())
        .all(&txn)
        .await
        .internal()?;
    txn.commit().await.internal()?;
    Ok(rows.into_iter().map(SchemaSummary::from).collect())
}

/// Marks every schema that follows `template_id` as having a pending upstream notification.
///
/// Sets `origin_updated_at` to `NULL` for all linked schemas referencing the template.
/// A merge sets it back to the current time.
pub(crate) async fn notify_upstream_change(
    conn: &impl ConnectionTrait,
    template_id: Uuid,
) -> Result<usize, YorishiroError> {
    use crate::models::_entities::schema_schemas::{Column, Entity};
    use sea_orm::EntityTrait;

    let result = Entity::update_many()
        .col_expr(Column::OriginUpdatedAt, Expr::value(None::<DateTime<Utc>>))
        .filter(Column::OriginTemplateId.eq(template_id))
        .filter(Column::OriginStatus.eq(SchemaOriginStatus::Linked.as_db_str()))
        .exec(conn)
        .await
        .internal()?;

    usize::try_from(result.rows_affected).internal()
}

/// Detaches every linked schema whose origin template matches `origin`.
///
/// Call it before deleting those templates.
/// It runs on a connection that is not scoped to one workspace, because the schemas live in other workspaces and tenants.
pub(crate) async fn detach_origins(
    conn: &impl ConnectionTrait,
    origin: impl sea_orm::sea_query::IntoCondition,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::schema_schemas::{Column, Entity};
    use sea_orm::EntityTrait;

    Entity::update_many()
        .col_expr(
            Column::OriginStatus,
            Expr::value(SchemaOriginStatus::Detached.as_db_str()),
        )
        .col_expr(
            Column::UpdatedAt,
            Expr::value(Some(crate::data::time::now())),
        )
        .filter(origin)
        .filter(Column::OriginStatus.eq(SchemaOriginStatus::Linked.as_db_str()))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// Creates the next version of a schema and archives the previous active one.
///
/// # Errors
/// Returns an error if the definition is invalid, the version number is taken or persistence fails.
pub async fn create_schema(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    workspace_id: Uuid,
    definition: MetaSchemaDefinition,
    origin_template_id: Option<Uuid>,
    origin_snapshot: Option<MetaSchemaDefinition>,
) -> Result<(SchemaRecord, VersioningDiff), YorishiroError> {
    use crate::models::_entities::schema_schemas::Column;

    validate_definition(&definition)?;
    let name = definition.name.clone();

    lock_version(conn, workspace_id, &name).await?;

    let previous = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Name.eq(&name))
        .filter(Column::Status.eq(SchemaStatus::Active.as_db_str()))
        .order_by_desc(Column::Version)
        .one(conn)
        .await
        .internal()?
        .map(SchemaRecord::try_from)
        .transpose()?;

    // A later version without an explicit origin inherits its predecessor's origin state, including a pending notification.
    // Creating a local version must not acknowledge an upstream update.
    let (origin_template_id, origin_snapshot, origin_updated_at) = match &previous {
        Some(previous) if origin_template_id.is_none() => (
            previous.origin_template_id,
            previous.origin_snapshot.clone(),
            previous.origin_updated_at,
        ),
        _ => (
            origin_template_id,
            origin_snapshot,
            if origin_template_id.is_some() {
                Some(crate::data::time::now())
            } else {
                None
            },
        ),
    };

    let latest_version = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Name.eq(&name))
        .order_by_desc(Column::Version)
        .one(conn)
        .await
        .internal()?
        .map_or(0, |row| row.version);

    let next_version = latest_version + 1;
    let diff = previous.as_ref().map_or_else(
        || VersioningDiff {
            is_breaking: false,
            reasons: Vec::new(),
        },
        |previous| metaschema::diff(&previous.definition, &definition),
    );

    if previous.is_some() {
        // `update_many` skips `before_save`, so `updated_at` is set explicitly.
        Entity::update_many()
            .col_expr(
                Column::Status,
                Expr::value(SchemaStatus::Archived.as_db_str()),
            )
            .col_expr(Column::UpdatedAt, Expr::value(crate::data::time::now()))
            .filter(Column::WorkspaceId.eq(workspace_id))
            .filter(Column::Name.eq(&name))
            .filter(Column::Status.eq(SchemaStatus::Active.as_db_str()))
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
        SchemaOriginStatus::Linked.as_db_str()
    } else {
        SchemaOriginStatus::Detached.as_db_str()
    };

    let active = ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        workspace_id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set(name.clone()),
        version: ActiveValue::Set(next_version),
        definition: ActiveValue::Set(definition_json),
        status: ActiveValue::Set(SchemaStatus::Active.as_db_str().to_string()),
        origin_template_id: ActiveValue::Set(origin_template_id),
        origin_status: ActiveValue::Set(origin_status.to_string()),
        origin_snapshot: ActiveValue::Set(origin_snapshot_json),
        origin_updated_at: ActiveValue::Set(origin_updated_at.map(Into::into)),
        ..Default::default()
    };
    let row = active.insert(conn).await.map_err(|err| {
        YorishiroError::conflict_on_unique(
            err,
            format!("schema '{name}' version {next_version} already exists (concurrent create?)"),
        )
    })?;

    // Inside the transaction, so a rolled-back insert leaves the workspace unchanged.
    crate::models::workspace_workspaces::mark_active(conn, workspace_id, row.id).await?;

    Ok((row.try_into()?, diff))
}

/// What a new schema is created from.
pub(crate) type CreateSchemaSource = crate::dtos::schemas::CreateSchemaRequest;

/// Archives a schema version.
///
/// # Errors
/// Returns the session, authorization or archive error.
pub(crate) async fn archive_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::access_keys::AuthContext,
    schema_id: Uuid,
) -> Result<SchemaRecord, YorishiroError> {
    crate::models::iam::session::WorkspaceSession::run(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Schema(schema_id),
        Some(crate::models::iam::policy::Action::ArchiveSchema),
        async |session| archive(session.txn(), session.workspace_id()?, schema_id).await,
    )
    .await
}

/// Deletes an archived schema version nothing references.
///
/// # Errors
/// Returns the session, authorization or delete error.
pub(crate) async fn delete_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::access_keys::AuthContext,
    schema_id: Uuid,
) -> Result<(), YorishiroError> {
    crate::models::iam::session::WorkspaceSession::run(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Schema(schema_id),
        Some(crate::models::iam::policy::Action::DeleteSchema),
        async |session| delete(session.txn(), session.workspace_id()?, schema_id).await,
    )
    .await
}

/// Creates a schema version from a definition or a template.
///
/// # Errors
/// Returns the session, authorization, template-resolution or create error.
pub(crate) async fn create_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::access_keys::AuthContext,
    source: CreateSchemaSource,
) -> Result<crate::dtos::schemas::CreateSchemaResponse, YorishiroError> {
    crate::models::iam::session::WorkspaceSession::run(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::CreateSchema,
        ),
        Some(crate::models::iam::policy::Action::CreateSchema),
        async |session| {
            let tenant_id = session.ctx.tenant_id;
            let mut origin_template_id = None;
            let mut origin_snapshot = None;
            let definition = match source {
                CreateSchemaSource::Definition(definition) => definition,
                CreateSchemaSource::Template { template_id } => {
                    let (definition, origin) =
                        crate::models::template_templates::resolve_template_definition(
                            &ctx.db,
                            tenant_id,
                            &template_id,
                        )
                        .await?;
                    origin_template_id = origin;
                    origin_snapshot = origin.map(|_| definition.clone());
                    definition
                }
            };
            let (schema, diff) = create_schema(
                session.txn(),
                tenant_id,
                session.workspace_id()?,
                definition,
                origin_template_id,
                origin_snapshot,
            )
            .await?;
            Ok(crate::dtos::schemas::CreateSchemaResponse { schema, diff })
        },
    )
    .await
}

/// Reads the active version of a schema by name.
///
/// # Errors
/// Returns the session, authorization or lookup error.
pub(crate) async fn get_active_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::access_keys::AuthContext,
    name: &str,
) -> Result<SchemaRecord, YorishiroError> {
    active_for_action(
        ctx,
        actor,
        name,
        crate::models::iam::policy::Action::GetActiveSchema,
    )
    .await
}

async fn active_for_action(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::access_keys::AuthContext,
    name: &str,
    action: crate::models::iam::policy::Action,
) -> Result<SchemaRecord, YorishiroError> {
    crate::models::iam::session::WorkspaceSession::read(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Permission(action),
        action,
        async |session| get_active_schema(session.txn(), session.workspace_id()?, name).await,
    )
    .await
}

/// Reads one schema version by ID.
///
/// # Errors
/// Returns the session, authorization or lookup error.
pub(crate) async fn get_by_id_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::access_keys::AuthContext,
    schema_id: Uuid,
) -> Result<SchemaRecord, YorishiroError> {
    crate::models::iam::session::WorkspaceSession::read(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Schema(schema_id),
        crate::models::iam::policy::Action::GetSchema,
        async |session| get_by_id(session.txn(), session.workspace_id()?, schema_id).await,
    )
    .await
}

/// The JSON Schema one entity type of the active schema validates against.
///
/// # Errors
/// Returns the lookup error, or not-found when the entity type is not in the schema.
pub(crate) async fn entity_type_json_schema_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::access_keys::AuthContext,
    name: &str,
    entity_type: &str,
) -> Result<serde_json::Value, YorishiroError> {
    let record = active_for_action(
        ctx,
        actor,
        name,
        crate::models::iam::policy::Action::GetEntityTypeJsonSchema,
    )
    .await?;
    let definition = record
        .definition
        .entity_types
        .get(entity_type)
        .ok_or_else(|| {
            YorishiroError::not_found(format!(
                "entity_type '{entity_type}' not found in schema '{name}'"
            ))
        })?;
    Ok(metaschema::entity_type_to_json_schema(definition))
}
