//! Durable state for asynchronous infer-fill jobs.

use crate::error::{ResultExt, YorishiroError};
use crate::models::inference_jobs::{ActiveModel, Column, Entity, Model};
use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set,
};
use uuid::Uuid;

pub const QUEUED: &str = "queued";
pub const RUNNING: &str = "running";
pub const COMPLETED: &str = "completed";
pub const FAILED: &str = "failed";

pub async fn create(
    conn: &impl ConnectionTrait,
    id: Uuid,
    workspace_id: Uuid,
    schema_name: &str,
) -> Result<(), YorishiroError> {
    let active = ActiveModel {
        id: Set(id),
        workspace_id: Set(workspace_id),
        schema_name: Set(schema_name.to_string()),
        status: Set(QUEUED.to_string()),
        applied: Set(0),
        skipped: Set(0),
        error: ActiveValue::Set(None),
        ..Default::default()
    };
    active.insert(conn).await.internal()?;
    Ok(())
}

pub async fn get(conn: &impl ConnectionTrait, id: Uuid) -> Result<Option<Model>, YorishiroError> {
    Entity::find_by_id(id).one(conn).await.internal()
}

/// Claims a queued job, or a queue delivery reaped after a worker restart.
/// The queue provider owns delivery exclusivity, while terminal updates remain conditional.
pub async fn claim(conn: &impl ConnectionTrait, id: Uuid) -> Result<bool, YorishiroError> {
    let result = Entity::update_many()
        .col_expr(Column::Status, Expr::value(RUNNING))
        .col_expr(Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(Column::Id.eq(id))
        .filter(Column::Status.is_in([QUEUED, RUNNING]))
        .exec(conn)
        .await
        .internal()?;
    Ok(result.rows_affected == 1)
}

pub async fn complete(
    conn: &impl ConnectionTrait,
    id: Uuid,
    applied: i64,
    skipped: i64,
) -> Result<(), YorishiroError> {
    Entity::update_many()
        .col_expr(Column::Status, Expr::value(COMPLETED))
        .col_expr(Column::Applied, Expr::value(applied))
        .col_expr(Column::Skipped, Expr::value(skipped))
        .col_expr(Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(Column::Id.eq(id))
        .filter(Column::Status.eq(RUNNING))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

pub async fn fail(
    conn: &impl ConnectionTrait,
    id: Uuid,
    message: &str,
) -> Result<(), YorishiroError> {
    Entity::update_many()
        .col_expr(Column::Status, Expr::value(FAILED))
        .col_expr(Column::Error, Expr::value(message))
        .col_expr(Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(Column::Id.eq(id))
        .filter(Column::Status.is_in([QUEUED, RUNNING]))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}
