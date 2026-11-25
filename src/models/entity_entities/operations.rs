//! Entity CRUD and listing, with the entity quota enforced under a workspace advisory lock.

use sea_orm::ConnectionTrait;
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveValue, QueryOrder, QuerySelect};
use uuid::Uuid;

use super::validation::{resolve_entity_type, validate_data};
use super::{
    ActiveModel, CreateEntityInput, Entity, EntityRecord, ListEntitiesQuery, UpdateEntityInput,
};
use crate::error::{ResultExt, YorishiroError};

/// Checks the workspace's `max_entities` cap before an insert.
/// `NULL` means unlimited.
async fn check_entity_quota(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    quota: Option<i32>,
) -> Result<(), YorishiroError> {
    let max_entities = crate::models::workspace_workspaces::Entity::find_by_id(workspace_id)
        .select_only()
        .column(crate::models::_entities::workspace_workspaces::Column::MaxEntities)
        .into_tuple::<Option<i32>>()
        .one(conn)
        .await
        .internal()?
        .flatten();

    let Some(max) = crate::data::quotas::effective_limit(max_entities, quota) else {
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
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn count(conn: &impl ConnectionTrait, workspace_id: Uuid) -> Result<i64, YorishiroError> {
    use crate::models::_entities::entity_entities::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .count(conn)
        .await
        .internal()
        .and_then(|count| i64::try_from(count).internal())
}

/// Creates a new entity: resolves the schema name to its currently active schema, checks that the `entity_type` exists in that version, validates `data`, and persists the result.
///
/// `created_by` is the acting user's ID, or `None` for an unattributed service/automation API key.
///
/// The quota check and insert are serialized with a workspace-scoped advisory lock: without it, concurrent creates could each read a count under `max_entities` and both insert, overshooting the cap.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn create(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    input: CreateEntityInput,
    created_by: Option<Uuid>,
) -> Result<EntityRecord, YorishiroError> {
    create_with_quota(conn, workspace_id, input, created_by, None).await
}

pub(crate) async fn create_with_quota(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    input: CreateEntityInput,
    created_by: Option<Uuid>,
    quota: Option<i32>,
) -> Result<EntityRecord, YorishiroError> {
    crate::db::lock_for_update(conn, &workspace_id.to_string())
        .await
        .internal()?;

    check_entity_quota(conn, workspace_id, quota).await?;

    // Before resolving the schema, so an empty workspace is told it is empty rather than reporting the schema name as not found.
    if crate::models::workspace_workspaces::is_schema_pending(conn, workspace_id).await? {
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

    crate::models::schema_schemas::lock_version(conn, workspace_id, &input.schema_name).await?;
    let schema =
        crate::models::schema_schemas::get_active_schema(conn, workspace_id, &input.schema_name)
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

    active.insert(conn).await.internal().map(EntityRecord::from)
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn get(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<EntityRecord, YorishiroError> {
    use crate::models::_entities::entity_entities::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(id))
        .into_model::<EntityRecord>()
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("entity '{id}' was not found")))
}

/// Fully replaces an existing entity's `data`.
///
/// Validation is done against the schema version the entity was actually created with (the row's `schema_id`), so existing entities don't silently break compatibility even if the active version has since moved on.
/// `updated_by` is the acting user's ID, or `None` for an unattributed service/automation API key.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn update(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    input: UpdateEntityInput,
) -> Result<EntityRecord, YorishiroError> {
    use crate::models::_entities::entity_entities::Column;

    let row = Entity::find_by_id(input.id)
        .filter(Column::WorkspaceId.eq(workspace_id))
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("entity '{}' was not found", input.id)))?;
    let current_token = row.embedding_sync_token.clone();
    let existing = EntityRecord::from(row);
    if let Some(expected) = input.expected_updated_at
        && existing.updated_at != expected
    {
        return Err(YorishiroError::Conflict {
            message: format!(
                "entity '{}' changed while it was being edited (it is now {}); re-read it and retry",
                input.id, existing.updated_at
            ),
        });
    }
    let schema =
        crate::models::schema_schemas::get_by_id(conn, workspace_id, existing.schema_id).await?;
    let entity_type_def = resolve_entity_type(&schema.definition, &existing.entity_type)?;
    validate_data(entity_type_def, &input.data)?;

    let active = ActiveModel {
        id: ActiveValue::Unchanged(input.id),
        data: ActiveValue::Set(input.data),
        updated_by: ActiveValue::Set(input.updated_by),
        ..Default::default()
    };
    if input.expected_updated_at.is_some() {
        // Bulk updates do not run model hooks, so retain the normal timestamp hook explicitly.
        let active = active.before_save(conn, false).await.internal()?;
        let mut rows = Entity::update_many()
            .set(active)
            .filter(Column::WorkspaceId.eq(workspace_id))
            .filter(Column::Id.eq(input.id))
            // Compare the same snapshot atomically without SQLite timestamp text-format differences.
            .filter(Column::EmbeddingSyncToken.eq(current_token))
            .exec_with_returning(conn)
            .await
            .internal()?;
        return rows
            .pop()
            .map(EntityRecord::from)
            .ok_or_else(|| YorishiroError::Conflict {
                message: format!(
                    "entity '{}' changed while it was being edited; re-read it and retry",
                    input.id
                ),
            });
    }
    active.update(conn).await.internal().map(EntityRecord::from)
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn delete(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::entity_entities::Column;

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

/// Lists workspace entities with optional JSONB containment filtering.
///
/// `query.filter` uses `sea_query::extension::postgres::PgExpr::contains` to build `data @> filter` without raw SQL.
/// `ColumnTrait::contains` instead builds a text `LIKE` expression.
///
/// SQLite does not support this filter (`BackendUnsupported` is returned) because `data` is stored as `TEXT` and the JSONB containment operator is a PostgreSQL-specific feature.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn list(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    query: ListEntitiesQuery,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    list_workspaces(conn, &[workspace_id], None, query).await
}

pub(crate) async fn list_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::api_keys::AuthContext,
    query: ListEntitiesQuery,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    use crate::models::iam::{access::read_workspaces, policy::Action};
    let workspaces = read_workspaces(&ctx.db, actor, Action::EntityList).await?;
    let txn = crate::db::begin_workspace_list(ctx, actor.tenant_id, &workspaces).await?;
    let records = list_workspaces(&txn, &workspaces, Some(actor.tenant_id), query).await?;
    txn.commit().await.internal()?;
    Ok(records)
}

pub(crate) async fn get_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::api_keys::AuthContext,
    id: Uuid,
) -> Result<EntityRecord, YorishiroError> {
    let inferred = actor.workspace_id.is_none();
    let mut actor = actor.clone();
    actor.workspace_id = Some(super::workspace_for_actor(&ctx.db, &actor, id).await?);
    let txn = crate::db::begin_workspace(ctx, actor.tenant_id, actor.require_workspace()?).await?;
    let _authorization = crate::models::iam::access::authorize_workspace_action(
        &txn,
        &ctx.db,
        &actor,
        crate::models::iam::policy::Action::EntityGet,
    )
    .await
    .map_err(|error| crate::models::iam::access::hide_inferred_resource(inferred, error))?;
    get(&txn, actor.require_workspace()?, id).await
}

pub(crate) async fn create_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::api_keys::AuthContext,
    input: CreateEntityInput,
) -> Result<EntityRecord, YorishiroError> {
    use crate::models::iam::{access, policy::Action};
    let mut actor = actor.clone();
    let workspace_id = access::select_workspace(&ctx.db, &actor, Action::EntityCreate).await?;
    actor.workspace_id = Some(workspace_id);
    let quotas = crate::data::quotas::for_tenant(ctx, actor.tenant_id).await?;
    let txn = crate::db::begin_workspace_write(ctx, actor.tenant_id, workspace_id).await?;
    let authorization =
        access::authorize_workspace_action(&txn, &ctx.db, &actor, Action::EntityCreate).await?;
    let record =
        create_with_quota(&txn, workspace_id, input, actor.user_id, quotas.entities).await?;
    crate::workers::embedding_sync::enqueue_after_write(ctx, workspace_id, record.id, &txn).await?;
    txn.commit().await.internal()?;
    if let Some(authorization) = authorization {
        authorization.commit().await.internal()?;
    }
    Ok(record)
}

pub(crate) async fn update_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::api_keys::AuthContext,
    mut input: UpdateEntityInput,
) -> Result<EntityRecord, YorishiroError> {
    let inferred = actor.workspace_id.is_none();
    let mut actor = actor.clone();
    actor.workspace_id = Some(super::workspace_for_actor(&ctx.db, &actor, input.id).await?);
    let workspace_id = actor.require_workspace()?;
    let txn = crate::db::begin_workspace_write(ctx, actor.tenant_id, workspace_id).await?;
    let authorization = crate::models::iam::access::authorize_workspace_action(
        &txn,
        &ctx.db,
        &actor,
        crate::models::iam::policy::Action::EntityUpdate,
    )
    .await
    .map_err(|error| crate::models::iam::access::hide_inferred_resource(inferred, error))?;
    input.updated_by = actor.user_id;
    let record = update(&txn, workspace_id, input).await?;
    crate::workers::embedding_sync::enqueue_after_write(ctx, workspace_id, record.id, &txn).await?;
    txn.commit().await.internal()?;
    if let Some(authorization) = authorization {
        authorization.commit().await.internal()?;
    }
    Ok(record)
}

pub(crate) async fn delete_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::api_keys::AuthContext,
    id: Uuid,
) -> Result<(), YorishiroError> {
    let inferred = actor.workspace_id.is_none();
    let mut actor = actor.clone();
    actor.workspace_id = Some(super::workspace_for_actor(&ctx.db, &actor, id).await?);
    let workspace_id = actor.require_workspace()?;
    let txn = crate::db::begin_workspace_write(ctx, actor.tenant_id, workspace_id).await?;
    let authorization = crate::models::iam::access::authorize_workspace_action(
        &txn,
        &ctx.db,
        &actor,
        crate::models::iam::policy::Action::EntityDelete,
    )
    .await
    .map_err(|error| crate::models::iam::access::hide_inferred_resource(inferred, error))?;
    delete(&txn, workspace_id, id).await?;
    txn.commit().await.internal()?;
    if let Some(authorization) = authorization {
        authorization.commit().await.internal()?;
    }
    Ok(())
}

async fn list_workspaces(
    conn: &impl ConnectionTrait,
    workspaces: &[Uuid],
    tenant_id: Option<Uuid>,
    query: ListEntitiesQuery,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    use sea_orm::sea_query::extension::postgres::PgExpr;

    use crate::models::_entities::entity_entities::Column;

    let mut select = Entity::find().filter(Column::WorkspaceId.is_in(workspaces.iter().copied()));
    if let Some(tenant_id) = tenant_id {
        select = select.filter(Column::WorkspaceId.in_subquery(
            crate::models::workspace_workspaces::ids_for_tenant(tenant_id),
        ));
    }
    if let Some(entity_type) = query.entity_type {
        select = select.filter(Column::EntityType.eq(entity_type));
    }
    if let Some(filter) = query.filter {
        if crate::db::is_sqlite(conn) {
            return Err(YorishiroError::BackendUnsupported {
                message: "entity filter (JSONB containment) is not supported on SQLite".to_string(),
            });
        }
        select = select.filter(Expr::col(Column::Data).contains(Expr::val(filter)));
    }
    if let Some(schema_version) = query.schema_version {
        select = select.filter(Column::SchemaVersion.eq(schema_version));
    }

    let select = select
        .order_by_desc(Column::CreatedAt)
        .order_by_desc(Column::Id)
        .limit(query.page.limit())
        .offset(query.page.offset());

    select
        .into_model::<EntityRecord>()
        .all(conn)
        .await
        .internal()
}
