//! Historical and terminal job arrangements, not worker execution APIs.

use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use uuid::Uuid;
use yorishiro::YorishiroError;
use yorishiro::edition::ee::models::inference_jobs::{self, InferenceJobStatus};
use yorishiro::error::ResultExt;
use yorishiro::models::_entities::{inference_jobs as jobs, inference_proposals as proposals};

pub(crate) async fn complete(
    conn: &impl ConnectionTrait,
    id: Uuid,
    applied: i64,
    skipped: i64,
) -> Result<(), YorishiroError> {
    let result = jobs::Entity::update_many()
        .col_expr(jobs::Column::Status, Expr::value("completed"))
        .col_expr(jobs::Column::Applied, Expr::value(applied))
        .col_expr(jobs::Column::Skipped, Expr::value(skipped))
        .col_expr(jobs::Column::UpdatedAt, Expr::value(chrono::Utc::now()))
        .filter(jobs::Column::Id.eq(id))
        .filter(jobs::Column::Status.eq("running"))
        .exec(conn)
        .await
        .internal()?;
    if result.rows_affected == 0 {
        inference_jobs::get(conn, id).await?;
    }
    Ok(())
}

pub(crate) async fn fail(
    conn: &impl ConnectionTrait,
    id: Uuid,
    message: &str,
) -> Result<(), YorishiroError> {
    let result = jobs::Entity::update_many()
        .col_expr(jobs::Column::Status, Expr::value("failed"))
        .col_expr(jobs::Column::Error, Expr::value(message))
        .col_expr(jobs::Column::UpdatedAt, Expr::value(chrono::Utc::now()))
        .filter(jobs::Column::Id.eq(id))
        .filter(jobs::Column::Status.is_in(["queued", "running"]))
        .exec(conn)
        .await
        .internal()?;
    let job = inference_jobs::get(conn, id).await?;
    if result.rows_affected > 0
        && let Some(job) = job
    {
        assert_eq!(job.status, InferenceJobStatus::Failed);
        proposals::Entity::update_many()
            .col_expr(proposals::Column::Status, Expr::value("discarded"))
            .col_expr(
                proposals::Column::UpdatedAt,
                Expr::value(chrono::Utc::now()),
            )
            .filter(proposals::Column::WorkspaceId.eq(job.workspace_id))
            .filter(proposals::Column::JobId.eq(id))
            .filter(proposals::Column::Status.eq("pending"))
            .exec(conn)
            .await
            .internal()?;
    }
    Ok(())
}

pub(crate) async fn record_batch(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
    schema_id: Uuid,
    schema_version: i32,
    fields: impl IntoIterator<Item = (Uuid, String, serde_json::Value)>,
) -> Result<i64, YorishiroError> {
    let job = inference_jobs::get(conn, job_id)
        .await?
        .expect("proposal fixture requires an existing job");
    yorishiro::edition::ee::models::inference_proposals::record_batch_attempt(
        conn,
        workspace_id,
        job_id,
        job.attempt,
        schema_id,
        schema_version,
        fields,
    )
    .await
}
