use chrono::Utc;
use loco_rs::bgworker::{JobStatus, QueueProvider};
use loco_rs::config::PostgresQueueConfig;
use sea_orm::sqlx;
use yorishiro::workers::queue::postgres::PostgresTimezoneQueue;

async fn provider() -> Option<PostgresTimezoneQueue> {
    let url = postgres_test_url()?;
    let config = PostgresQueueConfig {
        uri: url,
        dangerously_flush: false,
        enable_logging: false,
        max_connections: 3,
        min_connections: 1,
        connect_timeout: 5_000,
        idle_timeout: 5_000,
        poll_interval_sec: 1,
        num_workers: 0,
        reaper: None,
    };
    let timezone = "Asia/Tokyo".parse().unwrap();
    let provider = PostgresTimezoneQueue::connect(&config, timezone)
        .await
        .unwrap();
    provider.setup().await.unwrap();
    Some(provider)
}

fn ulid() -> String {
    ulid::Ulid::new().to_string()
}

async fn configured_provider() -> Option<PostgresTimezoneQueue> {
    let uri = postgres_test_url()?;
    let config = PostgresQueueConfig {
        uri,
        dangerously_flush: false,
        enable_logging: false,
        max_connections: 3,
        min_connections: 1,
        connect_timeout: 5_000,
        idle_timeout: 5_000,
        poll_interval_sec: 1,
        num_workers: 0,
        reaper: None,
    };
    let provider = PostgresTimezoneQueue::connect(&config, "Asia/Tokyo".parse().unwrap())
        .await
        .unwrap_or_else(|error| panic!("configured PostgreSQL queue unavailable: {error}"));
    provider.setup().await.unwrap();
    Some(provider)
}

fn postgres_test_url() -> Option<String> {
    std::env::var("YORISHIRO_POSTGRES_TEST_URL")
        .ok()
        .or_else(|| {
            std::env::var("DATABASE_URL")
                .ok()
                .filter(|url| url.starts_with("postgres://") || url.starts_with("postgresql://"))
        })
}

#[tokio::test]
async fn postgres_provider_lifecycle_metadata_admin_and_collision_contract() {
    let Some(provider) = provider().await else {
        return;
    };
    let prefix = format!("tztest-{}", ulid());
    let queue = &provider.pool;
    let id = provider
        .enqueue_with_id(
            format!("{prefix}-job"),
            serde_json::json!({"v":1}),
            None,
            Some(3),
            ulid(),
        )
        .await
        .unwrap();
    let row: (
        String,
        chrono::DateTime<Utc>,
        chrono::DateTime<Utc>,
        chrono::DateTime<Utc>,
    ) = sqlx::query_as("SELECT id,run_at,created_at,updated_at FROM pg_loco_queue WHERE id=$1")
        .bind(&id)
        .fetch_one(queue)
        .await
        .unwrap();
    assert_eq!(row.0, id);
    assert_eq!(row.2.timestamp(), row.3.timestamp());
    assert_eq!(row.1.timestamp(), row.2.timestamp());
    let collision = provider
        .enqueue_with_id(
            "replacement".into(),
            serde_json::json!({}),
            None,
            None,
            id.clone(),
        )
        .await;
    assert!(collision.is_err());
    let unchanged: String = sqlx::query_scalar("SELECT name FROM pg_loco_queue WHERE id=$1")
        .bind(&id)
        .fetch_one(queue)
        .await
        .unwrap();
    assert_eq!(unchanged, format!("{prefix}-job"));

    let batch_ids = provider
        .enqueue_batch(
            "batch-test".into(),
            None,
            vec![
                (serde_json::json!({"n":1}), None),
                (serde_json::json!({"n":2}), Some(7)),
            ],
            Some(vec!["tag-a".into()]),
        )
        .await
        .unwrap();
    assert_eq!(batch_ids.len(), 2);
    let batch: Vec<(String, serde_json::Value, i32)> =
        sqlx::query_as("SELECT id,tags,priority FROM pg_loco_queue WHERE id=ANY($1)")
            .bind(&batch_ids)
            .fetch_all(queue)
            .await
            .unwrap();
    assert_eq!(batch.len(), 2);
    assert!(
        batch
            .iter()
            .all(|(_, tags, _)| tags == &serde_json::json!(["tag-a"]))
    );
    assert!(batch.iter().any(|(_, _, priority)| *priority == 7));
    let mut tx = queue.begin().await.unwrap();
    let now = Utc::now();
    for (suffix, status, created) in [
        ("failed", "failed", now - chrono::Duration::days(4)),
        ("old", "completed", now - chrono::Duration::days(6)),
        ("young", "completed", now),
    ] {
        let job_id = ulid();
        sqlx::query("INSERT INTO pg_loco_queue (id,name,task_data,status,run_at,interval,tags,priority,created_at,updated_at) VALUES($1,$2,'{}',$3,$4,NULL,NULL,0,$5,$5)")
            .bind(&job_id).bind(format!("{prefix}-{suffix}")).bind(status).bind(now).bind(created).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();

    let jobs = provider.get_jobs(None, None).await.unwrap();
    assert!(jobs.iter().any(|j| j.id == id));
    provider
        .cancel_jobs_by_name(&format!("{prefix}-job"))
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM pg_loco_queue WHERE id=$1")
        .bind(&id)
        .fetch_one(queue)
        .await
        .unwrap();
    assert_eq!(status, "cancelled");
    provider.retry_failed(None).await.unwrap();
    let failed_status: String =
        sqlx::query_scalar("SELECT status FROM pg_loco_queue WHERE name=$1")
            .bind(format!("{prefix}-failed"))
            .fetch_one(queue)
            .await
            .unwrap();
    assert_eq!(failed_status, "queued");
    let failed_id: String = sqlx::query_scalar("SELECT id FROM pg_loco_queue WHERE name=$1")
        .bind(format!("{prefix}-failed"))
        .fetch_one(queue)
        .await
        .unwrap();
    sqlx::query("UPDATE pg_loco_queue SET status='processing',updated_at=NOW()-interval '1 hour' WHERE id=$1").bind(&failed_id).execute(queue).await.unwrap();
    provider.requeue(&0).await.unwrap();
    let recovered: String = sqlx::query_scalar("SELECT status FROM pg_loco_queue WHERE id=$1")
        .bind(&failed_id)
        .fetch_one(queue)
        .await
        .unwrap();
    assert_eq!(recovered, "queued");
    let statuses = vec![JobStatus::Queued];
    assert!(
        provider
            .get_jobs(Some(&statuses), None)
            .await
            .unwrap()
            .iter()
            .any(|job| job.id == batch_ids[0])
    );
    let old = vec![JobStatus::Completed];
    provider.clear_jobs_older_than(3, Some(&old)).await.unwrap();
    let remain: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_loco_queue WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
        .fetch_one(queue)
        .await
        .unwrap();
    assert!(remain >= 2);
    provider
        .clear_by_status(vec![JobStatus::Cancelled])
        .await
        .unwrap();
    provider.clear_jobs_older_than(-1, None).await.unwrap();
    assert_eq!(provider.describe(), "postgres queue");
    provider.ping().await.unwrap();
    provider.shutdown().unwrap();
    sqlx::query("DELETE FROM pg_loco_queue WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(queue)
        .await
        .unwrap();
    sqlx::query("DELETE FROM pg_loco_queue WHERE id=ANY($1)")
        .bind(&batch_ids)
        .execute(queue)
        .await
        .unwrap();
}

#[tokio::test]
async fn postgres_subsecond_due_claim_is_preserved() {
    let Some(provider) = provider().await else {
        return;
    };
    let pool = &provider.pool;
    let id = provider
        .enqueue_with_id(
            "tagged-zone-check".into(),
            serde_json::json!({"n":1}),
            Some(vec!["zone".into()]),
            Some(9),
            ulid(),
        )
        .await
        .unwrap();
    let due = Utc::now()
        .with_timezone(&chrono::FixedOffset::east_opt(9 * 3600).unwrap())
        .with_timezone(&Utc)
        - chrono::Duration::milliseconds(1);
    sqlx::query("UPDATE pg_loco_queue SET run_at=$1 WHERE id=$2")
        .bind(due)
        .bind(&id)
        .execute(pool)
        .await
        .unwrap();
    let (status, run_at): (String, chrono::DateTime<Utc>) =
        sqlx::query_as("SELECT status,run_at FROM pg_loco_queue WHERE id=$1")
            .bind(&id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(status, "queued");
    assert!(run_at < Utc::now());
    sqlx::query("DELETE FROM pg_loco_queue WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn postgres_production_pool_uses_configured_session_timezone() {
    let Some(provider) = configured_provider().await else {
        return;
    };
    let zone: String = sqlx::query_scalar("SHOW TIME ZONE")
        .fetch_one(&provider.pool)
        .await
        .unwrap();
    assert_eq!(zone, "Asia/Tokyo");
}
