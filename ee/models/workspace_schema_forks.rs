//! Workspace-owned schema fork metadata and immutable fork heads.

use crate::error::OptionExt;
use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::{Alias, Asterisk, Expr, Func, Query};
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::schema_schemas;
use crate::models::schema_schemas::metaschema::{MetaSchemaDefinition, validate_definition};

use crate::models::_entities::schema_schemas::Column as SchemaColumn;
use crate::models::_entities::workspace_schema_fork_heads as fork_heads;
pub use crate::models::_entities::workspace_schema_forks::{ActiveModel, Column, Entity, Model};

pub(crate) async fn workspace_for_actor(
    db: &impl ConnectionTrait,
    actor: &crate::models::access_keys::AuthContext,
    id: Uuid,
) -> Result<Uuid, YorishiroError> {
    if let Some(workspace_id) = actor.workspace_id {
        return Ok(workspace_id);
    }
    let fork = Entity::find_by_id(id)
        .one(db)
        .await
        .internal()?
        .or_not_found("schema fork was not found")?;
    crate::models::iam::access::check_workspace_target(db, actor, fork.workspace_id).await
}

pub(crate) async fn list_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::access_keys::AuthContext,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<ForkRecord>, YorishiroError> {
    use crate::models::iam::{access::read_workspaces, policy::Action};
    let workspaces = read_workspaces(&ctx.db, actor, Action::ListSchemaForks).await?;
    let txn = crate::db::begin_workspace_list(ctx, actor.tenant_id, &workspaces).await?;
    let rows = Entity::find()
        .filter(Column::TenantId.eq(actor.tenant_id))
        .filter(Column::WorkspaceId.is_in(workspaces))
        .order_by_asc(Column::CreatedAt)
        .order_by_asc(Column::Id)
        .limit(page.limit())
        .offset(page.offset())
        .all(&txn)
        .await
        .internal()?;
    let mut records = Vec::with_capacity(rows.len());
    for row in rows {
        records.push(to_record(&txn, row).await?);
    }
    txn.commit().await.internal()?;
    Ok(records)
}

#[derive(Clone, Serialize, schemars::JsonSchema)]
pub struct ForkRecord {
    #[serde(rename = "fork_id")]
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub source_workspace_id: Uuid,
    pub source_schema_id: Uuid,
    pub source_schema_version: i32,
    pub source_schema_name: String,
    pub fork_schema_id: Uuid,
    pub fork_schema_version: i32,
    pub customized: bool,
    pub upstream_version: Option<i32>,
    #[schemars(with = "serde_json::Value")]
    pub definition: MetaSchemaDefinition,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "CreateForkRequest")]
pub struct CreateInput {
    pub source_workspace_id: Uuid,
    pub source_schema_id: Uuid,
}

#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "UpdateForkRequest")]
pub struct UpdateInput {
    #[schemars(with = "Option<serde_json::Value>")]
    pub definition: Option<MetaSchemaDefinition>,
    pub action: Option<String>,
    #[serde(default)]
    pub force: bool,
    pub expected_fork_schema_id: Option<Uuid>,
    pub expected_source_schema_id: Option<Uuid>,
}

#[derive(FromQueryResult)]
struct ForkEdge {
    source_workspace_id: Uuid,
}

fn parse_definition(value: Json) -> Result<MetaSchemaDefinition, YorishiroError> {
    serde_json::from_value(value).internal()
}

async fn source_by_id<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    source_workspace_id: Uuid,
    source_schema_id: Uuid,
) -> Result<Option<schema_schemas::Model>, YorishiroError> {
    // PostgreSQL uses the scoped SECURITY DEFINER function so fork source reads can cross workspace RLS; SQLite is single-tenant.
    if crate::db::is_postgres(conn) {
        let query = Query::select()
            .column(Asterisk)
            .from_function(
                Func::cust(Alias::new("workspace_schema_fork_source_by_id")).args([
                    Expr::val(tenant_id),
                    Expr::val(source_workspace_id),
                    Expr::val(source_schema_id),
                ]),
                Alias::new("source"),
            )
            .to_owned();
        return schema_schemas::Model::find_by_statement(conn.get_database_backend().build(&query))
            .one(conn)
            .await
            .internal();
    }

    schema_schemas::Entity::find()
        .filter(SchemaColumn::Id.eq(source_schema_id))
        .filter(SchemaColumn::TenantId.eq(tenant_id))
        .filter(SchemaColumn::WorkspaceId.eq(source_workspace_id))
        .one(conn)
        .await
        .internal()
}

async fn latest_source_by_name<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    source_workspace_id: Uuid,
    source_schema_name: &str,
) -> Result<Option<schema_schemas::Model>, YorishiroError> {
    // Keep the PostgreSQL RLS crossing in its scoped database function; the SQLite query has no RLS boundary.
    if crate::db::is_postgres(conn) {
        let query = Query::select()
            .column(Asterisk)
            .from_function(
                Func::cust(Alias::new("workspace_schema_fork_latest_source")).args([
                    Expr::val(tenant_id),
                    Expr::val(source_workspace_id),
                    Expr::val(source_schema_name),
                ]),
                Alias::new("source"),
            )
            .to_owned();
        return schema_schemas::Model::find_by_statement(conn.get_database_backend().build(&query))
            .one(conn)
            .await
            .internal();
    }

    schema_schemas::Entity::find()
        .filter(SchemaColumn::TenantId.eq(tenant_id))
        .filter(SchemaColumn::WorkspaceId.eq(source_workspace_id))
        .filter(SchemaColumn::Name.eq(source_schema_name))
        .filter(SchemaColumn::Status.eq(schema_schemas::SchemaStatus::Active.as_db_str()))
        .order_by_desc(SchemaColumn::Version)
        .one(conn)
        .await
        .internal()
}

async fn fork_edges<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    workspace_id: Uuid,
) -> Result<Vec<Uuid>, YorishiroError> {
    // The PostgreSQL function traverses fork edges across workspace RLS; SQLite can read the table directly.
    if crate::db::is_postgres(conn) {
        let query = Query::select()
            .column(Alias::new("source_workspace_id"))
            .from_function(
                Func::cust(Alias::new("workspace_schema_fork_edges"))
                    .args([Expr::val(tenant_id), Expr::val(workspace_id)]),
                Alias::new("edge"),
            )
            .to_owned();
        return ForkEdge::find_by_statement(conn.get_database_backend().build(&query))
            .all(conn)
            .await
            .internal()
            .map(|edges| {
                edges
                    .into_iter()
                    .map(|edge| edge.source_workspace_id)
                    .collect()
            });
    }

    Entity::find()
        .filter(Column::TenantId.eq(tenant_id))
        .filter(Column::WorkspaceId.eq(workspace_id))
        .all(conn)
        .await
        .internal()
        .map(|rows| {
            rows.into_iter()
                .map(|row| row.source_workspace_id)
                .collect()
        })
}

async fn current_source<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    source_workspace_id: Uuid,
    source_schema_id: Uuid,
) -> Result<schema_schemas::Model, YorishiroError> {
    let source = source_by_id(conn, tenant_id, source_workspace_id, source_schema_id)
        .await?
        .or_not_found("source schema was not found")?;
    if source.status != schema_schemas::SchemaStatus::Active.as_db_str() {
        return Err(YorishiroError::ValidationFailed {
            message: "source_schema_id must name the active source schema".into(),
            details: vec![],
            hint: "use the current active schema version".into(),
        });
    }
    Ok(source)
}

async fn latest_source<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    source_workspace_id: Uuid,
    source_schema_name: &str,
) -> Result<schema_schemas::Model, YorishiroError> {
    latest_source_by_name(conn, tenant_id, source_workspace_id, source_schema_name)
        .await?
        .or_not_found("source schema was not found")
}

async fn get_fork<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    workspace_id: Uuid,
    fork_id: Uuid,
) -> Result<Model, YorishiroError> {
    Entity::find()
        .filter(Column::Id.eq(fork_id))
        .filter(Column::TenantId.eq(tenant_id))
        .filter(Column::WorkspaceId.eq(workspace_id))
        .one(conn)
        .await
        .internal()?
        .or_not_found(format!("schema fork '{fork_id}' was not found"))
}

async fn next_version<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    name: &str,
) -> Result<i32, YorishiroError> {
    let max = schema_schemas::Entity::find()
        .filter(SchemaColumn::WorkspaceId.eq(workspace_id))
        .filter(SchemaColumn::Name.eq(name))
        .order_by_desc(SchemaColumn::Version)
        .one(conn)
        .await
        .internal()?
        .map_or(0, |row| row.version);
    Ok(max + 1)
}

async fn insert_fork_head<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    workspace_id: Uuid,
    definition: &MetaSchemaDefinition,
) -> Result<schema_schemas::Model, YorishiroError> {
    validate_definition(definition)?;
    let name = definition.name.clone();
    schema_schemas::lock_version(conn, workspace_id, &name).await?;
    let version = next_version(conn, workspace_id, &name).await?;
    schema_schemas::ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        workspace_id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set(name),
        version: ActiveValue::Set(version),
        definition: ActiveValue::Set(serde_json::to_value(definition).internal()?),
        status: ActiveValue::Set(schema_schemas::SchemaStatus::Archived.as_db_str().into()),
        origin_template_id: ActiveValue::Set(None),
        origin_status: ActiveValue::Set(
            schema_schemas::SchemaOriginStatus::Detached
                .as_db_str()
                .into(),
        ),
        origin_snapshot: ActiveValue::Set(None),
        origin_updated_at: ActiveValue::Set(None),
        ..Default::default()
    }
    .insert(conn)
    .await
    .internal()
}

async fn to_record<C: ConnectionTrait>(conn: &C, row: Model) -> Result<ForkRecord, YorishiroError> {
    let head = schema_schemas::Entity::find_by_id(row.fork_schema_id)
        .one(conn)
        .await
        .internal()?
        .or_internal("fork head is missing")?;
    let source_active = latest_source_by_name(
        conn,
        row.tenant_id,
        row.source_workspace_id,
        &row.source_schema_name,
    )
    .await?;
    let upstream_version = source_active
        .filter(|source| source.version > row.source_schema_version)
        .map(|source| source.version);
    Ok(ForkRecord {
        id: row.id,
        tenant_id: row.tenant_id,
        workspace_id: row.workspace_id,
        source_workspace_id: row.source_workspace_id,
        source_schema_id: row.source_schema_id,
        source_schema_version: row.source_schema_version,
        source_schema_name: row.source_schema_name,
        fork_schema_id: row.fork_schema_id,
        fork_schema_version: head.version,
        customized: row.customized,
        upstream_version,
        definition: parse_definition(head.definition)?,
        created_at: row.created_at.into(),
        updated_at: row.updated_at.into(),
    })
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn get<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    workspace_id: Uuid,
    fork_id: Uuid,
) -> Result<ForkRecord, YorishiroError> {
    to_record(
        conn,
        get_fork(conn, tenant_id, workspace_id, fork_id).await?,
    )
    .await
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn create<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    workspace_id: Uuid,
    input: CreateInput,
) -> Result<ForkRecord, YorishiroError> {
    if input.source_workspace_id == workspace_id {
        return Err(YorishiroError::ValidationFailed {
            message: "a workspace cannot fork itself".into(),
            details: vec![],
            hint: "choose a schema from another workspace in the same tenant".into(),
        });
    }
    let target = crate::models::workspace_workspaces::get_workspace(conn, workspace_id).await?;
    if target.tenant_id != tenant_id {
        return Err(YorishiroError::not_found("workspace was not found"));
    }
    // A single transaction-wide lock makes cycle validation atomic even when
    // two requests try to create opposite edges at the same time.
    crate::db::lock_for_update(conn, "schema-fork-cycle")
        .await
        .internal()?;
    let source = current_source(
        conn,
        tenant_id,
        input.source_workspace_id,
        input.source_schema_id,
    )
    .await?;
    let mut pending = vec![input.source_workspace_id];
    let mut visited = std::collections::HashSet::new();
    while let Some(source_id) = pending.pop() {
        if !visited.insert(source_id) {
            continue;
        }
        if source_id == workspace_id {
            return Err(YorishiroError::Conflict {
                message: "creating this fork would create a workspace cycle".into(),
            });
        }
        pending.extend(fork_edges(conn, tenant_id, source_id).await?);
    }
    let head = insert_fork_head(
        conn,
        tenant_id,
        workspace_id,
        &parse_definition(source.definition.clone())?,
    )
    .await?;
    let row = ActiveModel {
        id: crate::db::generated_id(ActiveValue::NotSet),
        tenant_id: ActiveValue::Set(tenant_id),
        workspace_id: ActiveValue::Set(workspace_id),
        source_workspace_id: ActiveValue::Set(input.source_workspace_id),
        source_schema_id: ActiveValue::Set(source.id),
        source_schema_version: ActiveValue::Set(source.version),
        source_schema_name: ActiveValue::Set(source.name),
        fork_schema_id: ActiveValue::Set(head.id),
        customized: ActiveValue::Set(false),
        ..Default::default()
    };
    let row = row.insert(conn).await.map_err(|err| {
        YorishiroError::conflict_on_unique(
            err,
            "this workspace already has a fork for that source schema",
        )
    })?;
    fork_heads::ActiveModel {
        id: crate::db::generated_id(ActiveValue::NotSet),
        tenant_id: ActiveValue::Set(tenant_id),
        workspace_id: ActiveValue::Set(workspace_id),
        fork_id: ActiveValue::Set(row.id),
        schema_id: ActiveValue::Set(row.fork_schema_id),
        ..Default::default()
    }
    .insert(conn)
    .await
    .internal()?;
    to_record(conn, row).await
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn update<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    workspace_id: Uuid,
    fork_id: Uuid,
    input: UpdateInput,
) -> Result<ForkRecord, YorishiroError> {
    if input.definition.is_some() == input.action.is_some() {
        return Err(YorishiroError::ValidationFailed {
            message: "provide exactly one of definition or action=follow".into(),
            details: vec![],
            hint: "local updates use definition; synchronization uses action=follow".into(),
        });
    }
    crate::db::lock_for_update(conn, &format!("schema-fork:{workspace_id}:{fork_id}"))
        .await
        .internal()?;
    let fork = get_fork(conn, tenant_id, workspace_id, fork_id).await?;
    if let Some(expected) = input.expected_fork_schema_id
        && expected != fork.fork_schema_id
    {
        return Err(YorishiroError::Conflict {
            message: "the schema fork changed since it was read".into(),
        });
    }
    let (definition, source_update) = if let Some(definition) = input.definition {
        validate_definition(&definition)?;
        if definition.name != fork.source_schema_name {
            return Err(YorishiroError::ValidationFailed {
                message: "a fork definition cannot rename its source schema".into(),
                details: vec![],
                hint: "keep definition.name equal to source_schema_name".into(),
            });
        }
        (definition, None)
    } else {
        if input.action.as_deref() != Some("follow") {
            return Err(YorishiroError::ValidationFailed {
                message: "the only supported fork action is follow".into(),
                details: vec![],
                hint: "use action=follow".into(),
            });
        }
        if fork.customized && !input.force {
            return Err(YorishiroError::Conflict {
                message: "following would discard local fork edits; set force=true".into(),
            });
        }
        let source = latest_source(
            conn,
            tenant_id,
            fork.source_workspace_id,
            &fork.source_schema_name,
        )
        .await?;
        if let Some(expected) = input.expected_source_schema_id
            && expected != source.id
        {
            return Err(YorishiroError::Conflict {
                message: "the source schema changed since it was read".into(),
            });
        }
        (parse_definition(source.definition.clone())?, Some(source))
    };
    let previous_head_id = fork.fork_schema_id;
    let head = insert_fork_head(conn, tenant_id, workspace_id, &definition).await?;
    let mut active: ActiveModel = fork.into();
    active.fork_schema_id = ActiveValue::Set(head.id);
    active.customized = ActiveValue::Set(source_update.is_none());
    active.updated_at = ActiveValue::Set(crate::data::time::now().into());
    if let Some(source) = source_update {
        active.source_schema_id = ActiveValue::Set(source.id);
        active.source_schema_version = ActiveValue::Set(source.version);
    }
    let updated = Entity::update_many()
        .set(active)
        .filter(Column::Id.eq(fork_id))
        .filter(Column::ForkSchemaId.eq(previous_head_id))
        .exec(conn)
        .await
        .internal()?;
    if updated.rows_affected == 0 {
        return Err(YorishiroError::Conflict {
            message: "the schema fork changed since it was read".into(),
        });
    }
    fork_heads::ActiveModel {
        id: crate::db::generated_id(ActiveValue::NotSet),
        tenant_id: ActiveValue::Set(tenant_id),
        workspace_id: ActiveValue::Set(workspace_id),
        fork_id: ActiveValue::Set(fork_id),
        schema_id: ActiveValue::Set(head.id),
        ..Default::default()
    }
    .insert(conn)
    .await
    .internal()?;
    get(conn, tenant_id, workspace_id, fork_id).await
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn delete<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    workspace_id: Uuid,
    fork_id: Uuid,
) -> Result<(), YorishiroError> {
    crate::db::lock_for_update(conn, &format!("schema-fork:{workspace_id}:{fork_id}"))
        .await
        .internal()?;
    let fork = get_fork(conn, tenant_id, workspace_id, fork_id).await?;
    let head_ids = fork_heads::Entity::find()
        .filter(fork_heads::Column::ForkId.eq(fork.id))
        .filter(fork_heads::Column::TenantId.eq(tenant_id))
        .filter(fork_heads::Column::WorkspaceId.eq(workspace_id))
        .all(conn)
        .await
        .internal()?
        .into_iter()
        .map(|head| head.schema_id)
        .collect::<Vec<_>>();
    let referenced = crate::models::_entities::entity_entities::Entity::find()
        .filter(crate::models::_entities::entity_entities::Column::WorkspaceId.eq(workspace_id))
        .filter(crate::models::_entities::entity_entities::Column::SchemaId.is_in(head_ids))
        .count(conn)
        .await
        .internal()?
        > 0;
    if referenced {
        return Err(YorishiroError::Conflict {
            message: "the schema fork is referenced by an entity and cannot be detached".into(),
        });
    }
    Entity::delete_by_id(fork_id).exec(conn).await.internal()?;
    Ok(())
}

/// Reads one fork of the caller's workspace.
///
/// # Errors
/// Returns the session, authorization or lookup error.
pub(crate) async fn get_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::access_keys::AuthContext,
    fork_id: Uuid,
) -> Result<ForkRecord, YorishiroError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        workspace_for_actor(&ctx.db, &actor, fork_id).await?,
    );
    crate::models::iam::session::WorkspaceSession::read(
        ctx,
        actor,
        target,
        crate::models::iam::policy::Action::GetSchemaFork,
        async |session| {
            get(
                session.txn(),
                session.ctx.tenant_id,
                session.workspace_id()?,
                fork_id,
            )
            .await
        },
    )
    .await
}

/// Creates a fork in the caller's workspace.
///
/// # Errors
/// Returns the session, authorization or create error.
pub(crate) async fn create_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::access_keys::AuthContext,
    input: CreateInput,
) -> Result<ForkRecord, YorishiroError> {
    crate::models::iam::session::WorkspaceSession::run(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::CreateSchemaFork,
        ),
        Some(crate::models::iam::policy::Action::CreateSchemaFork),
        async |session| {
            create(
                session.txn(),
                session.ctx.tenant_id,
                session.workspace_id()?,
                input,
            )
            .await
        },
    )
    .await
}

/// Updates one fork of the caller's workspace.
///
/// # Errors
/// Returns the session, authorization or update error.
pub(crate) async fn update_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::access_keys::AuthContext,
    fork_id: Uuid,
    input: UpdateInput,
) -> Result<ForkRecord, YorishiroError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        workspace_for_actor(&ctx.db, &actor, fork_id).await?,
    );
    crate::models::iam::session::WorkspaceSession::run(
        ctx,
        actor,
        target,
        Some(crate::models::iam::policy::Action::UpdateSchemaFork),
        async |session| {
            update(
                session.txn(),
                session.ctx.tenant_id,
                session.workspace_id()?,
                fork_id,
                input,
            )
            .await
        },
    )
    .await
}

/// Deletes one fork of the caller's workspace.
///
/// # Errors
/// Returns the session, authorization or delete error.
pub(crate) async fn delete_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::access_keys::AuthContext,
    fork_id: Uuid,
) -> Result<(), YorishiroError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        workspace_for_actor(&ctx.db, &actor, fork_id).await?,
    );
    crate::models::iam::session::WorkspaceSession::run(
        ctx,
        actor,
        target,
        Some(crate::models::iam::policy::Action::DeleteSchemaFork),
        async |session| {
            delete(
                session.txn(),
                session.ctx.tenant_id,
                session.workspace_id()?,
                fork_id,
            )
            .await
        },
    )
    .await
}
