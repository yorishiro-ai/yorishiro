pub(crate) use super::_entities::queue_job_lifecycles::{ActiveModel, Column, Entity, Model};
use chrono::Utc;
use sea_orm::ActiveValue::Set;
use sea_orm::entity::prelude::*;
use sea_orm::{ExprTrait, IntoActiveModel, QuerySelect, TransactionTrait};

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
            admitted_at: Set(None),
            lease_until: Set(None),
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

    pub(crate) async fn defer(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: Option<i32>,
        error: &str,
    ) -> Result<(), DbErr> {
        let now = Utc::now().fixed_offset();
        let result = Entity::update_many()
            .col_expr(Column::Status, Expr::value("retrying"))
            .col_expr(Column::RetryAt, Expr::value(now))
            .col_expr(
                Column::LeaseUntil,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(Column::Error, Expr::value(Some(error.to_owned())))
            .filter(Column::Id.eq(id))
            .filter(if let Some(attempt) = attempt {
                Column::Attempt
                    .eq(attempt)
                    .and(Column::Status.eq("running"))
            } else {
                Column::Status.is_in(["queued", "retrying"])
            })
            .exec(db)
            .await?;
        tracing::warn!(lifecycle_id = %id, diagnostic = error, "queue job deferred for retry");
        if result.rows_affected == 0 {
            return Err(DbErr::RecordNotFound("active queue lease".into()));
        }
        Ok(())
    }

    pub(crate) async fn start(db: &DatabaseConnection, id: Uuid) -> Result<Admission, DbErr> {
        Self::start_at(db, id, Utc::now().fixed_offset()).await
    }

    pub(crate) async fn start_at(
        db: &DatabaseConnection,
        id: Uuid,
        now: DateTimeWithTimeZone,
    ) -> Result<Admission, DbErr> {
        let txn = db.begin().await?;
        let initial = Entity::find_by_id(id).lock_exclusive().one(&txn).await?;
        let Some(initial) = initial else {
            txn.rollback().await?;
            return Err(DbErr::RecordNotFound("queue lifecycle".into()));
        };
        if initial.status == "running" {
            if running_lease_is_live(initial.lease_until, now) {
                txn.rollback().await?;
                return Ok(Admission::Duplicate);
            }
            tracing::warn!(lifecycle_id = %id, "reclaiming expired queue lease");
        }
        if matches!(initial.status.as_str(), "completed" | "cancelled") {
            txn.rollback().await?;
            return Ok(Admission::Terminal);
        }
        let limit = initial.concurrency_limit;
        let key = initial.concurrency_key;
        if let Some(ref key) = key {
            crate::db::lock_for_update(&txn, &format!("queue-concurrency:{key}")).await?;
            if let Some(limit) = limit {
                let in_flight = Entity::find()
                    .filter(Column::ConcurrencyKey.eq(key.clone()))
                    .filter(Column::Status.eq("running"))
                    .filter(Column::Id.ne(id))
                    .count(&txn)
                    .await?;
                if in_flight >= u64::try_from(std::cmp::Ord::max(limit, 0)).unwrap_or(0) {
                    txn.rollback().await?;
                    return Ok(Admission::Saturated);
                }
            }
        }
        let recovered = initial.status == "running";
        let lease_until = now + chrono::Duration::minutes(5);
        let result = Entity::update_many()
            .col_expr(Column::Status, Expr::value("running"))
            .col_expr(Column::ClaimAt, Expr::value(now))
            .col_expr(Column::StartAt, Expr::value(now))
            .col_expr(Column::AdmittedAt, Expr::value(now))
            .col_expr(
                Column::FailedAt,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(
                Column::RetryAt,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(Column::LeaseUntil, Expr::value(lease_until))
            .col_expr(Column::Attempt, Expr::col(Column::Attempt).add(1))
            .filter(Column::Id.eq(id))
            .filter(if recovered {
                Column::Status.eq("running")
            } else {
                Column::Status.is_in(["queued", "failed", "retrying"])
            })
            .exec(&txn)
            .await?;
        txn.commit().await?;
        if result.rows_affected == 1 {
            tracing::info!(
                lifecycle_id = %id,
                queue_start_seconds = std::cmp::Ord::max(now.signed_duration_since(initial.enqueue_at).num_seconds(), 0),
                "queue job admitted"
            );
        }
        Ok(if result.rows_affected == 1 {
            if recovered {
                Admission::Recovered {
                    attempt: initial.attempt + 1,
                }
            } else {
                Admission::Started {
                    attempt: initial.attempt + 1,
                }
            }
        } else {
            Admission::Duplicate
        })
    }

    pub(crate) async fn finish(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: Option<i32>,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), DbErr> {
        let now = Utc::now().fixed_offset();
        let mut update = Entity::update_many()
            .col_expr(Column::Status, Expr::value(status))
            .col_expr(
                Column::LeaseUntil,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(Column::Error, Expr::value(error.map(str::to_owned)))
            .filter(Column::Id.eq(id));
        if let Some(attempt) = attempt {
            update = update.filter(Column::Attempt.eq(attempt));
        }
        match status {
            "completed" => update = update.col_expr(Column::CompletedAt, Expr::value(now)),
            "failed" => update = update.col_expr(Column::FailedAt, Expr::value(now)),
            "retrying" => update = update.col_expr(Column::RetryAt, Expr::value(now)),
            "cancelled" => update = update.col_expr(Column::CancelledAt, Expr::value(now)),
            _ => {}
        }
        let result = update.exec(db).await?;
        if result.rows_affected == 0 {
            return Err(DbErr::RecordNotFound("active queue lease".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    Started { attempt: i32 },
    Recovered { attempt: i32 },
    Duplicate,
    Saturated,
    Terminal,
}

impl Admission {
    pub(crate) fn attempt(self) -> Option<i32> {
        match self {
            Self::Started { attempt } | Self::Recovered { attempt } => Some(attempt),
            Self::Duplicate | Self::Saturated | Self::Terminal => None,
        }
    }
}

fn running_lease_is_live(
    lease_until: Option<DateTimeWithTimeZone>,
    now: DateTimeWithTimeZone,
) -> bool {
    lease_until.is_none_or(|lease| lease > now)
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
            admitted_at: Some(start_at),
            lease_until: Some(start_at + chrono::Duration::minutes(5)),
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

    #[test]
    fn live_lease_is_duplicate_but_expired_lease_is_recoverable() {
        let now = Utc.timestamp_opt(2_000, 0).single().unwrap().fixed_offset();
        assert!(super::running_lease_is_live(
            Some(now + chrono::Duration::seconds(1)),
            now
        ));
        assert!(!super::running_lease_is_live(
            Some(now - chrono::Duration::seconds(1)),
            now
        ));
        assert!(super::running_lease_is_live(None, now));
    }
}
