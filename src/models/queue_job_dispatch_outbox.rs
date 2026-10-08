use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Statement, TransactionTrait};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

#[derive(Clone)]
pub(crate) struct PendingDispatch {
    pub(crate) lifecycle_id: Uuid,
    pub(crate) job_name: String,
    pub(crate) worker_class: String,
    pub(crate) payload: Value,
    pub(crate) worker_name: String,
    pub(crate) queue_name: Option<String>,
    pub(crate) tags: Option<Vec<String>>,
}

pub(crate) async fn record(
    db: &DatabaseConnection,
    lifecycle: crate::models::queue_job_lifecycles::Enqueue<'_>,
    mut payload: Value,
    worker_name: String,
    queue_name: Option<String>,
    tags: Option<Vec<String>>,
) -> Result<(), DbErr> {
    let lifecycle_id = lifecycle.id;
    if let Value::Object(fields) = &mut payload {
        fields.insert(
            "lifecycle_id".into(),
            Value::String(lifecycle_id.to_string()),
        );
    }
    let tags = tags
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| DbErr::Custom(error.to_string()))?;
    let txn = db.begin().await?;
    crate::models::queue_job_lifecycles::Entity::record_enqueue(&txn, lifecycle).await?;
    txn.execute_raw(Statement::from_sql_and_values(
        txn.get_database_backend(),
        "INSERT INTO queue_job_dispatch_outbox (lifecycle_id, payload, worker_name, queue_name, tags) VALUES ($1, $2, $3, $4, $5)",
            [lifecycle_id.into(), payload.into(), worker_name.into(), queue_name.into(), tags.into()],
    ))
    .await?;
    txn.commit().await
}

pub(crate) async fn due(db: &impl ConnectionTrait) -> Result<Vec<PendingDispatch>, YorishiroError> {
    let backend = db.get_database_backend();
    let cooldown = match backend {
        sea_orm::DatabaseBackend::Sqlite => {
            "(o.last_attempt_at IS NULL OR o.last_attempt_at <= datetime('now', '-60 seconds'))"
        }
        _ => {
            "(o.last_attempt_at IS NULL OR o.last_attempt_at <= CURRENT_TIMESTAMP - INTERVAL '60 seconds')"
        }
    };
    let retry_due = match backend {
        sea_orm::DatabaseBackend::Sqlite => {
            "(l.retry_at IS NULL OR datetime(l.retry_at) <= CURRENT_TIMESTAMP)"
        }
        _ => "(l.retry_at IS NULL OR l.retry_at <= CURRENT_TIMESTAMP)",
    };
    db.query_all_raw(Statement::from_string(
        backend,
        format!(
            "SELECT l.id, l.job_name, l.worker_class, o.payload, o.worker_name, o.queue_name, o.tags \
         FROM queue_job_lifecycles l JOIN queue_job_dispatch_outbox o ON o.lifecycle_id = l.id \
         WHERE ((l.status = 'queued' AND l.provider_job_id IS NULL) OR l.status = 'retrying') \
           AND {retry_due} \
           AND {cooldown} \
         ORDER BY l.enqueue_at LIMIT 100"
        ),
    ))
    .await
    .internal()?
    .into_iter()
    .map(|row| {
        let payload = if backend == sea_orm::DatabaseBackend::Sqlite {
            serde_json::from_str::<Value>(&row.try_get::<String>("", "payload").internal()?)
                .internal()?
        } else {
            row.try_get("", "payload").internal()?
        };
        let tags = if backend == sea_orm::DatabaseBackend::Sqlite {
            row.try_get::<Option<String>>("", "tags")
                .internal()?
                .map(|json| serde_json::from_str(&json).internal())
                .transpose()?
        } else {
            row.try_get("", "tags").internal()?
        };
        Ok(PendingDispatch {
            lifecycle_id: row.try_get("", "id").internal()?,
            job_name: row.try_get("", "job_name").internal()?,
            worker_class: row.try_get("", "worker_class").internal()?,
            payload,
            worker_name: row.try_get("", "worker_name").internal()?,
            queue_name: row.try_get("", "queue_name").internal()?,
            tags,
        })
    })
    .collect()
}

pub(crate) async fn attempted(db: &impl ConnectionTrait, id: Uuid) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "UPDATE queue_job_dispatch_outbox SET last_attempt_at = CURRENT_TIMESTAMP WHERE lifecycle_id = $1",
        [id.into()],
    ))
    .await
    .map(|_| ())
}

pub(crate) async fn claim_due(db: &impl ConnectionTrait, id: Uuid) -> Result<bool, DbErr> {
    let backend = db.get_database_backend();
    let cooldown = if backend == sea_orm::DatabaseBackend::Sqlite {
        "last_attempt_at IS NULL OR last_attempt_at <= datetime('now', '-60 seconds')"
    } else {
        "last_attempt_at IS NULL OR last_attempt_at <= CURRENT_TIMESTAMP - INTERVAL '60 seconds'"
    };
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            backend,
            format!("UPDATE queue_job_dispatch_outbox SET last_attempt_at = CURRENT_TIMESTAMP WHERE lifecycle_id = $1 AND ({cooldown})"),
            [id.into()],
        ))
        .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn remove(db: &impl ConnectionTrait, id: Uuid) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "DELETE FROM queue_job_dispatch_outbox WHERE lifecycle_id = $1",
        [id.into()],
    ))
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use migration::MigratorTrait;
    use sea_orm::Database;

    use super::*;
    use crate::models::queue_job_lifecycles::Enqueue;

    #[tokio::test]
    async fn record_is_recoverable_until_attempted() {
        let path = tempfile::tempdir().unwrap().keep().join("outbox.sqlite3");
        let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .unwrap();
        migration::Migrator::up(&db, None).await.unwrap();
        let id = Uuid::now_v7();
        record(
            &db,
            Enqueue {
                id,
                job_name: "test",
                worker_class: crate::workers::embedding_sync::WorkerClass::Shared,
                workspace_id: None,
                plan: None,
                concurrency_key: Some("shared"),
                concurrency_limit: Some(1),
            },
            serde_json::json!({"value": 7}),
            "TestWorker".into(),
            None,
            Some(vec!["test-tag".into()]),
        )
        .await
        .unwrap();

        let rows = due(&db).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].lifecycle_id, id);
        assert_eq!(rows[0].payload["lifecycle_id"], id.to_string());
        assert_eq!(rows[0].worker_name, "TestWorker");
        assert_eq!(rows[0].tags, Some(vec!["test-tag".into()]));

        assert!(claim_due(&db, id).await.unwrap());
        assert!(!claim_due(&db, id).await.unwrap());
        assert!(due(&db).await.unwrap().is_empty());
    }
}
