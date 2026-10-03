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
/// `NULL` means unlimited, the default for the enterprise edition.
async fn check_entity_quota(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    let max_entities = crate::models::workspace_workspaces::Entity::find_by_id(workspace_id)
        .select_only()
        .column(crate::models::_entities::workspace_workspaces::Column::MaxEntities)
        .into_tuple::<Option<i32>>()
        .one(conn)
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

/// Counts how many entities a workspace holds, for quota enforcement and workspace-detail summaries.
pub async fn count(conn: &impl ConnectionTrait, workspace_id: Uuid) -> Result<i64, YorishiroError> {
    use crate::models::_entities::entity_entities::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .count(conn)
        .await
        .internal()
        .map(|n| n as i64)
}

/// Creates a new entity: resolves the schema name to its currently active schema, checks that the entity_type exists in that version, validates `data`, and persists the result.
/// `created_by` is the acting user's ID, or `None` for an unattributed service/automation API key.
///
/// The quota check and insert are serialized with a workspace-scoped advisory lock: without it, concurrent creates could each read a count under `max_entities` and both insert, overshooting the cap.
pub async fn create(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    input: CreateEntityInput,
    created_by: Option<Uuid>,
) -> Result<EntityRecord, YorishiroError> {
    crate::db::lock_for_update(conn, &workspace_id.to_string())
        .await
        .internal()?;

    check_entity_quota(conn, workspace_id).await?;

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

/// [`get`], batched: one query for every id instead of one query per id.
/// An id with no matching row (deleted, or belonging to another workspace) is simply absent from
/// the returned map, mirroring `entity_relations::neighbors_batch`'s own no-match-is-no-entry
/// convention rather than erroring.
pub async fn get_batch(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, EntityRecord>, YorishiroError> {
    use crate::models::_entities::entity_entities::Column;

    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let rows = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.is_in(ids.iter().copied()))
        .into_model::<EntityRecord>()
        .all(conn)
        .await
        .internal()?;

    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}

/// Fully replaces an existing entity's `data`.
/// Validation is done against the schema version the entity was actually created with (the row's `schema_id`), so existing entities don't silently break compatibility even if the active version has since moved on.
/// `updated_by` is the acting user's ID, or `None` for an unattributed service/automation API key.
pub async fn update(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    input: UpdateEntityInput,
) -> Result<EntityRecord, YorishiroError> {
    let existing = get(conn, workspace_id, input.id).await?;
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
    active.update(conn).await.internal().map(EntityRecord::from)
}

/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
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

/// `query.filter` (JSONB containment, `data @> filter`) is the one condition here `ColumnTrait` can't express (`ColumnTrait::contains` builds a `LIKE '%...%'`, unrelated to Postgres's `@>` operator), so it's built with `sea_query::extension::postgres::PgExpr::contains`, the builder for `PgBinOper::Contains`, instead of a raw SQL string.
pub async fn list(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    query: ListEntitiesQuery,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    use sea_orm::sea_query::extension::postgres::PgExpr;

    use crate::models::_entities::entity_entities::Column;

    let mut select = Entity::find().filter(Column::WorkspaceId.eq(workspace_id));
    if let Some(entity_type) = query.entity_type {
        select = select.filter(Column::EntityType.eq(entity_type));
    }
    if let Some(filter) = query.filter {
        select = select.filter(Expr::col(Column::Data).contains(Expr::val(filter)));
    }
    if let Some(schema_version) = query.schema_version {
        select = select.filter(Column::SchemaVersion.eq(schema_version));
    }

    let select = select
        .order_by_desc(Column::CreatedAt)
        .limit(query.page.limit() as u64)
        .offset(query.page.offset() as u64);

    select
        .into_model::<EntityRecord>()
        .all(conn)
        .await
        .internal()
}

/// Fetches every entity for the workspace, with no pagination limit, for a full-workspace data export.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn export_all(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    use crate::models::_entities::entity_entities::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(Column::CreatedAt)
        .into_model::<EntityRecord>()
        .all(conn)
        .await
        .internal()
}
