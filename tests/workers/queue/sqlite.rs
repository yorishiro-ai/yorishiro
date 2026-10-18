use chrono::{DateTime, SecondsFormat, Timelike, Utc};
use loco_rs::bgworker::{JobStatus, QueueProvider};
use loco_rs::config::SqliteQueueConfig;
use sea_orm::sqlx::{self, SqlitePool, sqlite::SqlitePoolOptions};
use serde_json::Value as JsonValue;
use tempfile::TempDir;

fn exact_parts(at: DateTime<Utc>) -> (i64, i32) {
    (at.timestamp(), at.timestamp_subsec_nanos() as i32)
}
use yorishiro::workers::queue::sqlite::SqliteTimezoneQueue;

fn ulid_id() -> String {
    ulid::Ulid::generate().to_string()
}

fn config(path: &std::path::Path) -> SqliteQueueConfig {
    loco_rs::config::SqliteQueueConfig {
        uri: format!("sqlite://{}?mode=rwc", path.display()),
        dangerously_flush: false,
        enable_logging: false,
        max_connections: 2,
        min_connections: 1,
        connect_timeout: 1000,
        idle_timeout: 1000,
        poll_interval_sec: 1,
        num_workers: 1,
        reaper: None,
    }
}

async fn provider() -> (TempDir, SqliteTimezoneQueue) {
    let dir = tempfile::tempdir().expect("queue tempdir");
    let provider = SqliteTimezoneQueue::connect(
        &config(&dir.path().join("queue.sqlite")),
        "Asia/Tokyo".parse().expect("IANA timezone"),
    )
    .await
    .expect("queue provider");
    (dir, provider)
}

async fn enqueue(
    provider: &SqliteTimezoneQueue,
    class: &str,
    data: JsonValue,
    tags: Option<Vec<String>>,
    priority: i32,
) -> String {
    let now = chrono::Utc::now();
    let (sec, nanos) = exact_parts(now);
    let id = ulid_id();
    let tags = tags.map(serde_json::to_value).transpose().unwrap();
    sqlx::query("INSERT INTO sqlt_loco_queue (id,task_data,name,status,run_at,run_at_epoch_seconds,run_at_epoch_nanos,interval,tags,priority,created_at,updated_at) VALUES (?,?,?,'queued',?,?,?,?,?,?,?,?)")
    .bind(&id).bind(data).bind(class).bind(now.with_timezone(&provider.timezone).to_rfc3339()).bind(sec).bind(nanos).bind(None::<i64>).bind(tags).bind(priority).bind(now.with_timezone(&provider.timezone).to_rfc3339()).bind(now.with_timezone(&provider.timezone).to_rfc3339()).execute(&provider.pool).await.unwrap();
    id
}

async fn insert_raw(pool: &SqlitePool, id: &str, run_at: &str) {
    sqlx::query("INSERT INTO sqlt_loco_queue (id,name,task_data,status,run_at) VALUES (?, 'test', '{}', 'queued', ?)")
        .bind(id).bind(run_at).execute(pool).await.expect("legacy row");
}

#[tokio::test]
async fn enqueue_and_batch_store_tokyo_wall_time_and_exact_instant_key() {
    let (_dir, provider) = provider().await;
    enqueue(&provider, "one", serde_json::json!({}), None, 0).await;
    enqueue(&provider, "two", serde_json::json!({}), None, 0).await;
    let rows: Vec<(String, i64, i32)> = sqlx::query_as::<_, (String, i64, i32)>(
        "SELECT run_at,run_at_epoch_seconds,run_at_epoch_nanos FROM sqlt_loco_queue ORDER BY id",
    )
    .fetch_all(&provider.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    for (stored, sec, nanos) in rows {
        let parsed = DateTime::parse_from_rfc3339(&stored)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(exact_parts(parsed), (sec, nanos));
        assert!(stored.ends_with("+09:00"));
    }
    let metadata: Vec<(String, String)> =
        sqlx::query_as("SELECT created_at,updated_at FROM sqlt_loco_queue")
            .fetch_all(&provider.pool)
            .await
            .unwrap();
    assert!(
        metadata
            .iter()
            .all(|(created, updated)| created.ends_with("+09:00") && updated.ends_with("+09:00"))
    );
}

#[tokio::test]
async fn historical_utc_and_offset_instants_are_keyed_without_changing_run_at() {
    let (_dir, provider) = provider().await;
    let instant = chrono::Utc::now() - chrono::Duration::seconds(30);
    let utc = instant.to_rfc3339_opts(SecondsFormat::AutoSi, true);
    let offset = instant
        .with_timezone(&"America/New_York".parse::<chrono_tz::Tz>().unwrap())
        .to_rfc3339_opts(SecondsFormat::AutoSi, false);
    insert_raw(&provider.pool, "old-utc", &utc).await;
    insert_raw(&provider.pool, "old-offset", &offset).await;
    let before = [utc, offset];
    let provider_pool = provider.pool.clone();
    let timezone = provider.timezone;
    let instant_key = exact_parts(instant);
    let job = yorishiro::workers::queue::sqlite::dequeue(&provider_pool, &[], timezone)
        .await
        .unwrap()
        .unwrap();
    let after: Vec<(String, Option<i64>, Option<i32>)> = sqlx::query_as("SELECT quote(run_at),run_at_epoch_seconds,run_at_epoch_nanos FROM sqlt_loco_queue WHERE id IN ('old-utc','old-offset') ORDER BY id")
        .fetch_all(&provider.pool).await.unwrap();
    assert_eq!(after[0].0, format!("'{}'", before[1]));
    assert_eq!(after[1].0, format!("'{}'", before[0]));
    assert_eq!(after.iter().filter(|(_, sec, _)| sec.is_some()).count(), 2);
    assert!(
        after
            .iter()
            .all(|(_, sec, nanos)| sec.zip(*nanos) == Some(instant_key))
    );
    assert_eq!(job.id, "old-offset");
    let next = yorishiro::workers::queue::sqlite::dequeue(&provider_pool, &[], timezone)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.id, "old-utc");
    let unkeyed: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlt_loco_queue WHERE run_at_epoch_seconds IS NULL AND run_at_legacy_checked=0")
        .fetch_one(&provider.pool).await.unwrap();
    assert_eq!(unkeyed, 0);
}

#[tokio::test]
async fn malformed_legacy_run_at_stays_queued_and_unmodified() {
    let (_dir, provider) = provider().await;
    insert_raw(&provider.pool, "bad", "not-a-timestamp").await;
    assert!(provider.get_jobs(None, None).await.unwrap().is_empty());
    let row: (String, String, Option<i64>, i64) = sqlx::query_as(
        "SELECT status,run_at,run_at_epoch_seconds,run_at_legacy_checked FROM sqlt_loco_queue WHERE id='bad'",
    )
    .fetch_one(&provider.pool)
    .await
    .unwrap();
    assert_eq!(row, ("queued".into(), "not-a-timestamp".into(), None, 0));
}

#[tokio::test]
async fn recurring_completion_and_failed_retry_preserve_configured_zone_and_exact_key() {
    let (_dir, provider) = provider().await;
    let id = enqueue(&provider, "test", serde_json::json!({}), None, 0).await;
    provider
        .enqueue(
            "recurring".into(),
            None,
            serde_json::json!({}),
            None,
            Some(0),
        )
        .await
        .unwrap();
    let recurring: (String, i64, i32) = sqlx::query_as(
        "SELECT run_at,run_at_epoch_seconds,run_at_epoch_nanos FROM sqlt_loco_queue WHERE id=?",
    )
    .bind(&id)
    .fetch_one(&provider.pool)
    .await
    .unwrap();
    assert!(recurring.0.ends_with("+09:00"));
    assert_eq!(
        exact_parts(
            DateTime::parse_from_rfc3339(&recurring.0)
                .unwrap()
                .with_timezone(&Utc)
        ),
        (recurring.1, recurring.2)
    );
    sqlx::query("UPDATE sqlt_loco_queue SET status='failed' WHERE id=?")
        .bind(&id)
        .execute(&provider.pool)
        .await
        .unwrap();
    provider.retry_failed(Some(&id)).await.unwrap();
    let retried: (String, i64, i32) = sqlx::query_as(
        "SELECT run_at,run_at_epoch_seconds,run_at_epoch_nanos FROM sqlt_loco_queue WHERE id=?",
    )
    .bind(&id)
    .fetch_one(&provider.pool)
    .await
    .unwrap();
    assert!(retried.0.ends_with("+09:00"));
    assert_eq!(
        exact_parts(
            DateTime::parse_from_rfc3339(&retried.0)
                .unwrap()
                .with_timezone(&Utc)
        ),
        (retried.1, retried.2)
    );
}

#[tokio::test]
async fn queue_state_transitions_write_tokyo_timestamps() {
    let (_dir, provider) = provider().await;
    let timezone: chrono_tz::Tz = "Asia/Tokyo".parse().unwrap();
    let id = enqueue(&provider, "transition", serde_json::json!({}), None, 0).await;
    let claimed = provider
        .get_jobs(None, None)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(claimed.id, id);
    let claimed_at: String =
        sqlx::query_scalar("SELECT updated_at FROM sqlt_loco_queue WHERE id=?")
            .bind(&id)
            .fetch_one(&provider.pool)
            .await
            .unwrap();
    assert!(claimed_at.ends_with("+09:00"));

    sqlx::query("UPDATE sqlt_loco_queue SET status='failed' WHERE id=?")
        .bind(&id)
        .execute(&provider.pool)
        .await
        .unwrap();
    let failed_at: String = sqlx::query_scalar("SELECT updated_at FROM sqlt_loco_queue WHERE id=?")
        .bind(&id)
        .fetch_one(&provider.pool)
        .await
        .unwrap();
    assert!(failed_at.ends_with("+09:00"));

    provider.retry_failed(Some(&id)).await.unwrap();
    let old_updated = chrono::Utc::now() - chrono::Duration::minutes(10);
    let (old_sec, old_nanos) = exact_parts(old_updated);
    sqlx::query("UPDATE sqlt_loco_queue SET status='processing', updated_at=?, updated_at_epoch_seconds=?, updated_at_epoch_nanos=? WHERE id=?")
        .bind(old_updated.with_timezone(&timezone).to_rfc3339())
        .bind(old_sec)
        .bind(old_nanos)
        .bind(&id)
        .execute(&provider.pool)
        .await
        .unwrap();
    provider.requeue(&1).await.unwrap();
    let requeued: (String, String) =
        sqlx::query_as("SELECT status,updated_at FROM sqlt_loco_queue WHERE id=?")
            .bind(&id)
            .fetch_one(&provider.pool)
            .await
            .unwrap();
    assert_eq!(requeued.0, "queued");
    assert!(requeued.1.ends_with("+09:00"));

    provider.cancel_jobs_by_name("transition").await.unwrap();
    let cancelled: (String, String) =
        sqlx::query_as("SELECT status,updated_at FROM sqlt_loco_queue WHERE id=?")
            .bind(&id)
            .fetch_one(&provider.pool)
            .await
            .unwrap();
    assert_eq!(cancelled.0, "cancelled");
    assert!(cancelled.1.ends_with("+09:00"));
}

#[tokio::test]
async fn age_based_admin_and_reaper_comparisons_accept_offset_timestamps() {
    let (_dir, provider) = provider().await;
    let timezone: chrono_tz::Tz = "Asia/Tokyo".parse().unwrap();
    let id = enqueue(&provider, "old", serde_json::json!({}), None, 0).await;
    let old = chrono::Utc::now() - chrono::Duration::days(5);
    let old_local = old.with_timezone(&timezone).to_rfc3339();
    sqlx::query(
        "UPDATE sqlt_loco_queue SET created_at=?, updated_at=?, status='processing' WHERE id=?",
    )
    .bind(&old_local)
    .bind(&old_local)
    .bind(&id)
    .execute(&provider.pool)
    .await
    .unwrap();
    let statuses = vec![JobStatus::Processing];
    assert_eq!(
        provider
            .get_jobs(Some(&statuses), Some(2))
            .await
            .unwrap()
            .len(),
        1
    );
    provider
        .clear_jobs_older_than(2, Some(&statuses))
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlt_loco_queue WHERE id=?")
        .bind(&id)
        .fetch_one(&provider.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    let second = enqueue(&provider, "reaper", serde_json::json!({}), None, 0).await;
    sqlx::query("UPDATE sqlt_loco_queue SET updated_at=?, status='processing' WHERE id=?")
        .bind(&old_local)
        .bind(&second)
        .execute(&provider.pool)
        .await
        .unwrap();
    provider.requeue(&(4 * 24 * 60)).await.unwrap();
    let reaped: (String, String) =
        sqlx::query_as("SELECT status,updated_at FROM sqlt_loco_queue WHERE id=?")
            .bind(&second)
            .fetch_one(&provider.pool)
            .await
            .unwrap();
    assert_eq!(reaped.0, "queued");
    assert!(reaped.1.ends_with("+09:00"));
}

#[tokio::test]
async fn age_listing_and_clear_compare_offset_created_at_as_instants() {
    let (_dir, provider) = provider().await;
    let timezone: chrono_tz::Tz = "Asia/Tokyo".parse().unwrap();
    let cutoff = chrono::Utc::now() - chrono::Duration::days(2);
    let old_instant = cutoff - chrono::Duration::milliseconds(10);
    let new_instant = cutoff + chrono::Duration::milliseconds(10);
    let old_id = ulid_id();
    let new_id = ulid_id();
    for (id, created_at, created_instant) in [
        (
            &old_id,
            old_instant
                .with_timezone(&chrono::FixedOffset::east_opt(-5 * 3600).unwrap())
                .to_rfc3339(),
            old_instant,
        ),
        (
            &new_id,
            new_instant
                .with_timezone(&chrono::FixedOffset::east_opt(-5 * 3600).unwrap())
                .to_rfc3339(),
            new_instant,
        ),
    ] {
        sqlx::query("INSERT INTO sqlt_loco_queue (id,name,task_data,status,run_at,created_at,updated_at,created_at_epoch_seconds,created_at_epoch_nanos,created_at_legacy_checked,updated_at_epoch_seconds,updated_at_epoch_nanos,updated_at_legacy_checked) VALUES (?, 'age-instant', '{}', 'completed', ?, ?, ?, ?, ?, 1, ?, ?, 1)")
            .bind(id)
            .bind(chrono::Utc::now().with_timezone(&timezone).to_rfc3339())
            .bind(&created_at)
            .bind(&created_at)
            .bind(exact_parts(created_instant).0)
            .bind(exact_parts(created_instant).1)
            .bind(exact_parts(created_instant).0)
            .bind(exact_parts(created_instant).1)
            .execute(&provider.pool)
            .await
            .unwrap();
    }
    let statuses = vec![JobStatus::Completed];
    let cutoff = exact_parts(old_instant + chrono::Duration::milliseconds(10));
    let listed = yorishiro::workers::queue::sqlite::get_jobs_at_cutoff(
        &provider.pool,
        Some(&statuses),
        cutoff.0,
        cutoff.1,
    )
    .await
    .unwrap();
    assert!(listed.iter().any(|job| job.id == old_id));
    assert!(!listed.iter().any(|job| job.id == new_id));
    yorishiro::workers::queue::sqlite::clear_jobs_through(
        &provider.pool,
        cutoff.0,
        cutoff.1,
        Some(&statuses),
    )
    .await
    .unwrap();
    let remaining: Vec<String> =
        sqlx::query_scalar("SELECT id FROM sqlt_loco_queue WHERE name='age-instant' ORDER BY id")
            .fetch_all(&provider.pool)
            .await
            .unwrap();
    assert_eq!(remaining, vec![new_id]);
}

#[tokio::test]
async fn reaper_and_age_boundaries_keep_submillisecond_newer_rows() {
    let (_dir, provider) = provider().await;
    let timezone: chrono_tz::Tz = "Asia/Tokyo".parse().unwrap();
    let now = chrono::Utc::now().with_nanosecond(321_654_987).unwrap();
    let created_old = ulid_id();
    let created_new = ulid_id();
    let processing_old = ulid_id();
    let processing_new = ulid_id();
    let created_cutoff = now - chrono::Duration::days(2);
    let updated_cutoff = now - chrono::Duration::minutes(10);

    for (id, created, updated, status) in [
        (
            &created_old,
            created_cutoff - chrono::Duration::milliseconds(10),
            now,
            "completed",
        ),
        (
            &created_new,
            created_cutoff + chrono::Duration::milliseconds(10),
            now,
            "completed",
        ),
        (
            &processing_old,
            now,
            updated_cutoff - chrono::Duration::milliseconds(10),
            "processing",
        ),
        (
            &processing_new,
            now,
            updated_cutoff + chrono::Duration::milliseconds(10),
            "processing",
        ),
    ] {
        sqlx::query("INSERT INTO sqlt_loco_queue (id,name,task_data,status,run_at,created_at,updated_at,run_at_epoch_seconds,run_at_epoch_nanos,run_at_legacy_checked,created_at_epoch_seconds,created_at_epoch_nanos,created_at_legacy_checked,updated_at_epoch_seconds,updated_at_epoch_nanos,updated_at_legacy_checked) VALUES (?, 'sub-ms', '{}', ?, ?, ?, ?, ?, ?, 1, ?, ?, 1, ?, ?, 1)")
            .bind(id)
            .bind(status)
            .bind(now.with_timezone(&timezone).to_rfc3339())
            .bind(created.with_timezone(&timezone).to_rfc3339())
            .bind(updated.with_timezone(&timezone).to_rfc3339())
            .bind(exact_parts(now).0)
            .bind(exact_parts(now).1)
            .bind(exact_parts(created).0)
            .bind(exact_parts(created).1)
            .bind(exact_parts(updated).0)
            .bind(exact_parts(updated).1)
            .execute(&provider.pool)
            .await
            .unwrap();
    }

    let completed = vec![JobStatus::Completed];
    let (cutoff_sec, cutoff_nanos) = exact_parts(created_cutoff);
    let listed = yorishiro::workers::queue::sqlite::get_jobs_at_cutoff(
        &provider.pool,
        Some(&completed),
        cutoff_sec,
        cutoff_nanos,
    )
    .await
    .unwrap();
    assert!(listed.iter().any(|job| job.id == created_old));
    assert!(!listed.iter().any(|job| job.id == created_new));
    yorishiro::workers::queue::sqlite::clear_jobs_through(
        &provider.pool,
        cutoff_sec,
        cutoff_nanos,
        Some(&completed),
    )
    .await
    .unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlt_loco_queue WHERE id=?")
        .bind(&created_new)
        .fetch_one(&provider.pool)
        .await
        .unwrap();
    assert_eq!(remaining, 1);

    let (requeue_sec, requeue_nanos) = exact_parts(updated_cutoff);
    let requeue_now = chrono::Utc::now();
    yorishiro::workers::queue::sqlite::requeue_through(
        &provider.pool,
        requeue_sec,
        requeue_nanos,
        requeue_now
            .with_timezone(&timezone)
            .to_rfc3339_opts(SecondsFormat::AutoSi, false),
        exact_parts(requeue_now).0,
        exact_parts(requeue_now).1,
    )
    .await
    .unwrap();
    let statuses: Vec<(String, String)> =
        sqlx::query_as("SELECT id,status FROM sqlt_loco_queue WHERE id IN (?,?) ORDER BY id")
            .bind(&processing_new)
            .bind(&processing_old)
            .fetch_all(&provider.pool)
            .await
            .unwrap();
    let statuses = statuses
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        statuses.get(&processing_new).map(String::as_str),
        Some("processing")
    );
    assert_eq!(
        statuses.get(&processing_old).map(String::as_str),
        Some("queued")
    );
}

#[tokio::test]
async fn exact_epoch_due_query_uses_priority_ordered_index_without_table_scan() {
    let (_dir, provider) = provider().await;
    for i in 0..2000 {
        let status = if i % 4 == 0 { "completed" } else { "queued" };
        sqlx::query("INSERT INTO sqlt_loco_queue (id,name,task_data,status,run_at,priority,run_at_epoch_seconds,run_at_epoch_nanos) VALUES (?, 'fixture', '{}', ?, '2026-10-17 00:00:00', ?, ?, ?)")
            .bind(format!("fixture-{i:05}"))
            .bind(status)
            .bind(i % 17)
            .bind(1_900_000_000_i64 + i as i64)
            .bind(i % 1_000_000_000)
            .execute(&provider.pool)
            .await
            .unwrap();
    }
    sqlx::query("ANALYZE")
        .execute(&provider.pool)
        .await
        .unwrap();
    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as("EXPLAIN QUERY PLAN SELECT id FROM sqlt_loco_queue WHERE status='queued' AND run_at_epoch_seconds IS NOT NULL AND run_at_epoch_nanos IS NOT NULL AND run_at_epoch_seconds <= 2000000000 AND (run_at_epoch_seconds < 2000000000 OR run_at_epoch_nanos <= 1) ORDER BY priority DESC,run_at_epoch_seconds,run_at_epoch_nanos,id LIMIT 1")
        .fetch_all(&provider.pool).await.unwrap();
    let details = plan
        .iter()
        .map(|row| row.3.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        details.contains("idx_sqlt_queue_status_priority_epoch"),
        "expected exact priority-ordered range index, plan: {details}"
    );
    assert!(
        !details.contains("SCAN sqlt_loco_queue"),
        "unexpected full queue scan, plan: {details}"
    );
}

#[tokio::test]
async fn legacy_offset_after_utc_lexical_cutoff_is_keyed_and_eventually_dequeued() {
    let (_dir, provider) = provider().await;
    let instant = chrono::Utc::now() - chrono::Duration::seconds(30);
    let stored = instant
        .with_timezone(&"Pacific/Kiritimati".parse::<chrono_tz::Tz>().unwrap())
        .to_rfc3339_opts(SecondsFormat::AutoSi, false);
    assert!(stored > chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
    insert_raw(&provider.pool, "offset-late", &stored).await;
    let job = yorishiro::workers::queue::sqlite::dequeue(&provider.pool, &[], provider.timezone)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.id, "offset-late");
    let saved: (String, i64, i32) = sqlx::query_as("SELECT quote(run_at),run_at_epoch_seconds,run_at_epoch_nanos FROM sqlt_loco_queue WHERE id='offset-late'")
        .fetch_one(&provider.pool).await.unwrap();
    assert_eq!(saved.0, format!("'{}'", stored));
    assert_eq!((saved.1, saved.2), exact_parts(instant));
}

#[tokio::test]
async fn setup_migrates_legacy_schema_idempotently_and_contract_admin_methods_work() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            format!("sqlite://{}?mode=rwc", path.display())
                .parse()
                .unwrap(),
        )
        .await
        .unwrap();
    sqlx::query("CREATE TABLE sqlt_loco_queue (id TEXT NOT NULL, name TEXT NOT NULL, task_data JSON NOT NULL, status TEXT NOT NULL DEFAULT 'queued', run_at TIMESTAMP NOT NULL, interval INTEGER, created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, tags JSON)")
        .execute(&pool).await.unwrap();
    insert_raw(&pool, "legacy-setup", "2020-01-01 00:00:00").await;
    sqlx::query("ALTER TABLE sqlt_loco_queue ADD COLUMN run_at_epoch_seconds INTEGER")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE sqlt_loco_queue ADD COLUMN run_at_epoch_nanos INTEGER")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "ALTER TABLE sqlt_loco_queue ADD COLUMN run_at_legacy_checked INTEGER NOT NULL DEFAULT 0",
    )
    .execute(&pool)
    .await
    .unwrap();
    let timezone: chrono_tz::Tz = "Asia/Tokyo".parse().unwrap();
    let config = config(&path);
    let provider = SqliteTimezoneQueue::connect(&config, timezone)
        .await
        .unwrap();
    provider.setup().await.unwrap();
    provider.setup().await.unwrap();

    let legacy: (i32, Option<i64>, Option<i32>, i64) = sqlx::query_as("SELECT priority,run_at_epoch_seconds,run_at_epoch_nanos,run_at_legacy_checked FROM sqlt_loco_queue WHERE id='legacy-setup'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(legacy.0, 0);
    assert_eq!(legacy.1, None);
    assert_eq!(legacy.2, None);
    assert_eq!(legacy.3, 0);
    let job = yorishiro::workers::queue::sqlite::dequeue(&pool, &[], timezone)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.id, "legacy-setup");
    let legacy_key: Option<i64> = sqlx::query_scalar(
        "SELECT run_at_epoch_seconds FROM sqlt_loco_queue WHERE id='legacy-setup'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        legacy_key,
        Some(
            exact_parts(
                DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
            .0
        )
    );

    let first = ulid_id();
    let second = ulid_id();
    for (id, name, status) in [
        (&first, "admin-a", "queued"),
        (&second, "admin-b", "failed"),
    ] {
        sqlx::query("INSERT INTO sqlt_loco_queue (id,name,task_data,status,run_at,created_at,updated_at) VALUES (?,?,'{}',?,'2020-01-01 00:00:00','2020-01-01 00:00:00','2020-01-01 00:00:00')")
            .bind(id).bind(name).bind(status).execute(&pool).await.unwrap();
    }
    let queued = vec![JobStatus::Queued];
    let jobs = provider.get_jobs(Some(&queued), Some(2)).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, first);

    provider
        .clear_by_status(vec![JobStatus::Failed])
        .await
        .unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlt_loco_queue WHERE id=?")
        .bind(&second)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
    provider.cancel_jobs_by_name("admin-a").await.unwrap();
    let cancelled: String = sqlx::query_scalar("SELECT status FROM sqlt_loco_queue WHERE id=?")
        .bind(&first)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cancelled, "cancelled");
    provider.clear().await.unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlt_loco_queue")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
    provider.ping().await.unwrap();
    assert_eq!(provider.describe(), "sqlite queue");
    provider.shutdown().unwrap();
}
