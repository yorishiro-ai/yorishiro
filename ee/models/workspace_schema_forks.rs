//! Workspace-owned schema fork metadata and immutable fork heads.

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::{MetaSchemaDefinition, VersioningDiff, validate_definition};
use crate::models::schema_schemas;

use crate::models::_entities::schema_schemas::Column as SchemaColumn;
pub use crate::models::_entities::workspace_schema_forks::{ActiveModel, Column, Entity, Model};

#[derive(Clone, Debug, Serialize)]
pub struct ForkRecord {
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
    pub definition: MetaSchemaDefinition,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateInput {
    pub source_workspace_id: Uuid,
    pub source_schema_id: Uuid,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpdateInput {
    pub definition: Option<MetaSchemaDefinition>,
    pub action: Option<String>,
    #[serde(default)]
    pub force: bool,
    pub expected_fork_schema_id: Option<Uuid>,
    pub expected_source_schema_id: Option<Uuid>,
}

fn parse_definition(value: Json) -> Result<MetaSchemaDefinition, YorishiroError> {
    serde_json::from_value(value).internal()
}

async fn current_source<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    source_workspace_id: Uuid,
    source_schema_id: Uuid,
) -> Result<schema_schemas::Model, YorishiroError> {
    let source = schema_schemas::Entity::find()
        .filter(SchemaColumn::Id.eq(source_schema_id))
        .filter(SchemaColumn::TenantId.eq(tenant_id))
        .filter(SchemaColumn::WorkspaceId.eq(source_workspace_id))
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found("source schema was not found"))?;
    if source.status != "active" {
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
    schema_schemas::Entity::find()
        .filter(SchemaColumn::TenantId.eq(tenant_id))
        .filter(SchemaColumn::WorkspaceId.eq(source_workspace_id))
        .filter(SchemaColumn::Name.eq(source_schema_name))
        .filter(SchemaColumn::Status.eq("active"))
        .order_by_desc(SchemaColumn::Version)
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found("source schema was not found"))
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
        .ok_or_else(|| YorishiroError::not_found(format!("schema fork '{fork_id}' was not found")))
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
        .map(|row| row.version)
        .unwrap_or(0);
    Ok(max + 1)
}

async fn insert_fork_head<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    workspace_id: Uuid,
    definition: &MetaSchemaDefinition,
) -> Result<(schema_schemas::Model, VersioningDiff), YorishiroError> {
    validate_definition(definition)?;
    let name = definition.name.clone();
    let version = next_version(conn, workspace_id, &name).await?;
    let diff = VersioningDiff {
        is_breaking: false,
        reasons: Vec::new(),
    };
    let row = schema_schemas::ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        workspace_id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set(name),
        version: ActiveValue::Set(version),
        definition: ActiveValue::Set(serde_json::to_value(definition).internal()?),
        status: ActiveValue::Set("archived".into()),
        origin_template_id: ActiveValue::Set(None),
        origin_status: ActiveValue::Set(schema_schemas::ORIGIN_STATUS_DETACHED.into()),
        origin_snapshot: ActiveValue::Set(None),
        origin_updated_at: ActiveValue::Set(None),
        ..Default::default()
    }
    .insert(conn)
    .await
    .internal()?;
    Ok((row, diff))
}

async fn to_record<C: ConnectionTrait>(conn: &C, row: Model) -> Result<ForkRecord, YorishiroError> {
    let head = schema_schemas::Entity::find_by_id(row.fork_schema_id)
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("fork head is missing")))?;
    let source_active = schema_schemas::Entity::find()
        .filter(SchemaColumn::TenantId.eq(row.tenant_id))
        .filter(SchemaColumn::WorkspaceId.eq(row.source_workspace_id))
        .filter(SchemaColumn::Name.eq(&row.source_schema_name))
        .filter(SchemaColumn::Status.eq("active"))
        .order_by_desc(SchemaColumn::Version)
        .one(conn)
        .await
        .internal()?;
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

pub async fn list<C: ConnectionTrait>(
    conn: &C,
    tenant_id: Uuid,
    workspace_id: Uuid,
) -> Result<Vec<ForkRecord>, YorishiroError> {
    let rows = Entity::find()
        .filter(Column::TenantId.eq(tenant_id))
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(Column::CreatedAt)
        .all(conn)
        .await
        .internal()?;
    let mut records = Vec::with_capacity(rows.len());
    for row in rows {
        records.push(to_record(conn, row).await?);
    }
    Ok(records)
}

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
    let target = crate::models::tenancy::get_workspace(conn, workspace_id).await?;
    if target.tenant_id != tenant_id {
        return Err(YorishiroError::not_found("workspace was not found"));
    }
    let source_workspace =
        crate::models::tenancy::get_workspace(conn, input.source_workspace_id).await?;
    if source_workspace.tenant_id != tenant_id {
        return Err(YorishiroError::not_found("source workspace was not found"));
    }
    crate::db::lock_for_update(
        conn,
        &format!("schema-fork:{workspace_id}:{}", input.source_workspace_id),
    )
    .await
    .internal()?;
    let source = current_source(
        conn,
        tenant_id,
        input.source_workspace_id,
        input.source_schema_id,
    )
    .await?;
    let head = insert_fork_head(
        conn,
        tenant_id,
        workspace_id,
        &parse_definition(source.definition.clone())?,
    )
    .await?
    .0;
    let row = ActiveModel {
        id: crate::db::sqlite_generated_id(conn, ActiveValue::NotSet),
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
        if matches!(
            err.sql_err(),
            Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
        ) {
            YorishiroError::Conflict {
                message: "this workspace already has a fork for that source schema".into(),
            }
        } else {
            YorishiroError::Internal(err.into())
        }
    })?;
    to_record(conn, row).await
}

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
    let head = insert_fork_head(conn, tenant_id, workspace_id, &definition)
        .await?
        .0;
    let mut active: ActiveModel = fork.into();
    active.fork_schema_id = ActiveValue::Set(head.id);
    active.customized = ActiveValue::Set(source_update.is_none());
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
    get(conn, tenant_id, workspace_id, fork_id).await
}

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
    let referenced = crate::models::_entities::entity_entities::Entity::find()
        .filter(crate::models::_entities::entity_entities::Column::WorkspaceId.eq(workspace_id))
        .filter(crate::models::_entities::entity_entities::Column::SchemaId.eq(fork.fork_schema_id))
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
