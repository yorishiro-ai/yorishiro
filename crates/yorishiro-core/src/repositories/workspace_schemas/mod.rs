use chrono::{DateTime, Utc};
use sea_query::{Alias, Expr, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::{MetaSchemaDefinition, validate_definition};
use crate::repositories::schemas;

pub use crate::models::workspace_schemas::*;

#[derive(Iden)]
enum WorkspaceSchemas {
    Table,
    Id,
    WorkspaceId,
    SourceId,
    SourceVersion,
    Definition,
    Customized,
    CreatedAt,
    UpdatedAt,
}

#[derive(sqlx::FromRow)]
struct WorkspaceSchemaRow {
    id: Uuid,
    workspace_id: Uuid,
    source_id: Uuid,
    source_version: i32,
    definition: Value,
    customized: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl WorkspaceSchemaRow {
    fn into_record(self) -> Result<WorkspaceSchemaRecord, YorishiroError> {
        let definition = serde_json::from_value(self.definition).internal()?;
        Ok(WorkspaceSchemaRecord {
            id: self.id,
            workspace_id: self.workspace_id,
            source_id: self.source_id,
            source_version: self.source_version,
            definition,
            customized: self.customized,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

fn workspace_schema_columns() -> [WorkspaceSchemas; 8] {
    [
        WorkspaceSchemas::Id,
        WorkspaceSchemas::WorkspaceId,
        WorkspaceSchemas::SourceId,
        WorkspaceSchemas::SourceVersion,
        WorkspaceSchemas::Definition,
        WorkspaceSchemas::Customized,
        WorkspaceSchemas::CreatedAt,
        WorkspaceSchemas::UpdatedAt,
    ]
}

/// Fetches a workspace's fork, or `None` when it has not forked. A workspace without a fork
/// uses its tenant's schema directly, which is the behaviour every workspace had before forks
/// existed.
pub async fn get(
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<Option<WorkspaceSchemaRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(workspace_schema_columns())
        .from((Alias::new("content"), WorkspaceSchemas::Table))
        .and_where(Expr::col(WorkspaceSchemas::WorkspaceId).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);
    let row: Option<WorkspaceSchemaRow> = sqlx::query_as_with(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .internal()?;

    row.map(WorkspaceSchemaRow::into_record).transpose()
}

/// Forks the tenant's currently active schema of `schema_name` into this workspace.
///
/// The fork starts as an exact copy, so `customized` is false and the workspace behaves as it
/// did before forking until something is actually edited.
pub async fn fork(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    workspace_id: Uuid,
    schema_name: &str,
) -> Result<WorkspaceSchemaRecord, YorishiroError> {
    let source = schemas::get_active_schema(&mut *conn, tenant_id, schema_name).await?;
    let definition_json = serde_json::to_value(&source.definition).internal()?;

    let (sql, values) = Query::insert()
        .into_table((Alias::new("content"), WorkspaceSchemas::Table))
        .columns([
            WorkspaceSchemas::WorkspaceId,
            WorkspaceSchemas::SourceId,
            WorkspaceSchemas::SourceVersion,
            WorkspaceSchemas::Definition,
        ])
        .values_panic([
            workspace_id.into(),
            source.id.into(),
            source.version.into(),
            definition_json.into(),
        ])
        .returning(Query::returning().columns(workspace_schema_columns()))
        .build_sqlx(PostgresQueryBuilder);

    let row: WorkspaceSchemaRow = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .map_err(|err| {
            if err
                .as_database_error()
                .is_some_and(|db_err| db_err.is_unique_violation())
            {
                YorishiroError::Conflict {
                    message: format!("workspace '{workspace_id}' has already forked its schema"),
                }
            } else {
                YorishiroError::Internal(err.into())
            }
        })?;

    row.into_record()
}

/// Replaces a fork's definition with the caller's edit.
///
/// Marks the fork `customized`, which is what later tells a follow-the-master operation that
/// overwriting would discard someone's work rather than replace an untouched copy.
pub async fn update_definition(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    definition: MetaSchemaDefinition,
) -> Result<WorkspaceSchemaRecord, YorishiroError> {
    validate_definition(&definition)?;
    let definition_json = serde_json::to_value(&definition).internal()?;

    let (sql, values) = Query::update()
        .table((Alias::new("content"), WorkspaceSchemas::Table))
        .values([
            (WorkspaceSchemas::Definition, definition_json.into()),
            (WorkspaceSchemas::Customized, true.into()),
            (
                WorkspaceSchemas::UpdatedAt,
                Expr::current_timestamp().into(),
            ),
        ])
        .and_where(Expr::col(WorkspaceSchemas::WorkspaceId).eq(workspace_id))
        .returning(Query::returning().columns(workspace_schema_columns()))
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<WorkspaceSchemaRow> = sqlx::query_as_with(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .internal()?;

    match row {
        Some(row) => row.into_record(),
        None => Err(YorishiroError::not_found(format!(
            "workspace '{workspace_id}' has not forked its schema"
        ))),
    }
}

/// Whether the tenant's schema has moved past the version this fork was taken from.
///
/// Compares recorded versions rather than diffing definitions: a fork is behind when its source
/// has a newer active version, whatever the two definitions happen to contain.
pub async fn upstream_version(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    fork: &WorkspaceSchemaRecord,
) -> Result<Option<i32>, YorishiroError> {
    let source = schemas::get_by_id(&mut *conn, tenant_id, fork.source_id).await?;
    let active = schemas::get_active_schema(&mut *conn, tenant_id, &source.name).await?;
    Ok((active.version > fork.source_version).then_some(active.version))
}

/// Replaces a fork with the tenant's current active schema, discarding any local edits.
///
/// `force` must be set for a `customized` fork. Overwriting local edits is a decision only the
/// caller can make, and doing it silently is how someone's schema work disappears -- so an
/// un-forced attempt is refused rather than resolved one way or the other.
pub async fn follow_upstream(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    workspace_id: Uuid,
    force: bool,
) -> Result<WorkspaceSchemaRecord, YorishiroError> {
    let fork = get(&mut *conn, workspace_id).await?.ok_or_else(|| {
        YorishiroError::not_found(format!(
            "workspace '{workspace_id}' has not forked its schema"
        ))
    })?;

    if fork.customized && !force {
        return Err(YorishiroError::Conflict {
            message: "this workspace has edited its schema; following the tenant's schema would \
                      discard those edits"
                .into(),
        });
    }

    let source = schemas::get_by_id(&mut *conn, tenant_id, fork.source_id).await?;
    let active = schemas::get_active_schema(&mut *conn, tenant_id, &source.name).await?;
    let definition_json = serde_json::to_value(&active.definition).internal()?;

    let (sql, values) = Query::update()
        .table((Alias::new("content"), WorkspaceSchemas::Table))
        .values([
            (WorkspaceSchemas::SourceId, active.id.into()),
            (WorkspaceSchemas::SourceVersion, active.version.into()),
            (WorkspaceSchemas::Definition, definition_json.into()),
            (WorkspaceSchemas::Customized, false.into()),
            (
                WorkspaceSchemas::UpdatedAt,
                Expr::current_timestamp().into(),
            ),
        ])
        .and_where(Expr::col(WorkspaceSchemas::WorkspaceId).eq(workspace_id))
        .returning(Query::returning().columns(workspace_schema_columns()))
        .build_sqlx(PostgresQueryBuilder);

    let row: WorkspaceSchemaRow = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .internal()?;

    row.into_record()
}

/// Drops a workspace's fork, returning it to its tenant's schema.
pub async fn unfork(conn: &mut PgConnection, workspace_id: Uuid) -> Result<(), YorishiroError> {
    let (sql, values) = Query::delete()
        .from_table((Alias::new("content"), WorkspaceSchemas::Table))
        .and_where(Expr::col(WorkspaceSchemas::WorkspaceId).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);
    let result = sqlx::query_with(&sql, values)
        .execute(&mut *conn)
        .await
        .internal()?;

    if result.rows_affected() == 0 {
        return Err(YorishiroError::not_found(format!(
            "workspace '{workspace_id}' has not forked its schema"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/repositories/workspace_schemas/mod.rs"]
mod tests;
