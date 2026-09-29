pub(crate) use super::_entities::queue_job_lifecycles::{ActiveModel, Column, Entity, Model};
use chrono::Utc;
use sea_orm::ActiveValue::Set;
use sea_orm::entity::prelude::*;
use sea_orm::{ExprTrait, IntoActiveModel, TransactionTrait};

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if !insert && self.updated_at.is_unchanged() {
            let mut this = self;
            this.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
            Ok(this)
        } else {
            Ok(self)
        }
    }
}

impl Entity {
    pub(crate) async fn record_enqueue(
        db: &impl ConnectionTrait,
        enqueue: Enqueue<'_>,
    ) -> Result<(), DbErr> {
        ActiveModel {
            id: Set(enqueue.id),
            provider_job_id: Set(None),
            job_name: Set(enqueue.job_name.to_owned()),
            worker_class: Set(enqueue.worker_class.to_owned()),
            workspace_id: Set(enqueue.workspace_id),
            plan: Set(enqueue.plan.map(str::to_owned)),
            status: Set("queued".to_owned()),
            enqueue_at: Set(Utc::now().fixed_offset()),
            claim_at: Set(None),
            start_at: Set(None),
            retry_at: Set(None),
            completed_at: Set(None),
            failed_at: Set(None),
            cancelled_at: Set(None),
            attempt: Set(0),
            concurrency_key: Set(enqueue.concurrency_key.map(str::to_owned)),
            concurrency_limit: Set(enqueue.concurrency_limit),
            error: Set(None),
            ..Default::default()
        }
        .insert(db)
        .await
        .map(|_| ())
    }

    pub(crate) async fn mark_dispatched(
        db: &impl ConnectionTrait,
        id: Uuid,
        provider_job_id: &str,
    ) -> Result<(), DbErr> {
        let mut row = Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(DbErr::RecordNotFound("queue lifecycle".into()))?
            .into_active_model();
        row.provider_job_id = Set(Some(provider_job_id.to_owned()));
        row.update(db).await.map(|_| ())
    }

    pub(crate) async fn start(db: &DatabaseConnection, id: Uuid) -> Result<bool, DbErr> {
        let initial = Entity::find_by_id(id).one(db).await?;
        let Some(initial) = initial else {
            return Err(DbErr::RecordNotFound("queue lifecycle".into()));
        };
        let limit = initial.concurrency_limit;
        let key = initial.concurrency_key;
        let txn = db.begin().await?;
        if let Some(ref key) = key {
            crate::db::lock_for_update(&txn, &format!("queue-concurrency:{key}")).await?;
            if let Some(limit) = limit {
                let in_flight = Entity::find()
                    .filter(Column::ConcurrencyKey.eq(key.clone()))
                    .filter(Column::Status.eq("running"))
                    .count(&txn)
                    .await?;
                if in_flight >= u64::try_from(std::cmp::Ord::max(limit, 0)).unwrap_or(0) {
                    txn.rollback().await?;
                    return Ok(false);
                }
            }
        }
        let now = Utc::now().fixed_offset();
        let result = Entity::update_many()
            .col_expr(Column::Status, Expr::value("running"))
            .col_expr(Column::ClaimAt, Expr::value(now))
            .col_expr(Column::StartAt, Expr::value(now))
            .col_expr(
                Column::FailedAt,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(
                Column::RetryAt,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(Column::Attempt, Expr::col(Column::Attempt).add(1))
            .filter(Column::Id.eq(id))
            .filter(Column::Status.is_in(["queued", "failed", "retrying"]))
            .exec(&txn)
            .await?;
        txn.commit().await?;
        Ok(result.rows_affected == 1)
    }

    pub(crate) async fn finish(
        db: &impl ConnectionTrait,
        id: Uuid,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), DbErr> {
        let mut row = Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(DbErr::RecordNotFound("queue lifecycle".into()))?
            .into_active_model();
        row.status = Set(status.to_owned());
        row.error = Set(error.map(str::to_owned));
        let now = Utc::now().fixed_offset();
        match status {
            "completed" => row.completed_at = Set(Some(now)),
            "failed" => row.failed_at = Set(Some(now)),
            "retrying" => row.retry_at = Set(Some(now)),
            "cancelled" => row.cancelled_at = Set(Some(now)),
            _ => {}
        }
        row.update(db).await.map(|_| ())
    }
}

pub(crate) struct Enqueue<'a> {
    pub(crate) id: Uuid,
    pub(crate) job_name: &'a str,
    pub(crate) worker_class: &'a str,
    pub(crate) workspace_id: Option<Uuid>,
    pub(crate) plan: Option<&'a str>,
    pub(crate) concurrency_key: Option<&'a str>,
    pub(crate) concurrency_limit: Option<i32>,
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    #[test]
    fn queue_start_latency_uses_recorded_timestamps() {
        let enqueue_at = Utc.timestamp_opt(1_000, 0).single().unwrap().fixed_offset();
        let start_at = Utc.timestamp_opt(1_075, 0).single().unwrap().fixed_offset();
        let row = crate::models::queue_job_lifecycles::Model {
            id: uuid::Uuid::nil(),
            provider_job_id: None,
            job_name: "job".into(),
            worker_class: "shared".into(),
            workspace_id: None,
            plan: None,
            status: "running".into(),
            enqueue_at,
            claim_at: Some(start_at),
            start_at: Some(start_at),
            retry_at: None,
            completed_at: None,
            failed_at: None,
            cancelled_at: None,
            attempt: 1,
            concurrency_key: None,
            concurrency_limit: None,
            error: None,
            created_at: enqueue_at,
            updated_at: start_at,
        };
        assert_eq!(
            row.start_at
                .map(|start| (start - row.enqueue_at).num_seconds()),
            Some(75)
        );
    }
}
