use chrono::Utc;
use loco_rs::bgworker::{Job, JobStatus, QueueProvider};
use loco_rs::config::RedisQueueConfig;
use redis::{AsyncCommands, Client, Script};
use std::time::Duration;
use yorishiro::workers::queue::valkey::ValkeyTimezoneQueue;

const QUEUE: &str = "queue:";
const JOB: &str = "job:";
const PROCESSING: &str = "processing:";
const FAILED: &str = "failed:";

fn score(priority: i32) -> f64 {
    priority as f64
}

fn job_json(
    id: String,
    name: String,
    data: serde_json::Value,
    interval: Option<i64>,
    priority: i32,
    _timezone: chrono_tz::Tz,
) -> Result<(Job, String), serde_json::Error> {
    let run_at = Utc::now();
    let job = Job {
        id,
        name,
        data,
        status: JobStatus::Queued,
        run_at,
        interval,
        created_at: Some(run_at),
        updated_at: Some(run_at),
        tags: None,
        priority,
    };
    Ok((job.clone(), serde_json::to_string(&job)?))
}

fn test_client() -> Client {
    let uri = std::env::var("YORISHIRO_VALKEY_TEST_URL")
        .expect("YORISHIRO_VALKEY_TEST_URL must explicitly select a Valkey test database");
    let parsed = reqwest::Url::parse(&uri).expect("valid Valkey test URL");
    assert_eq!(parsed.scheme(), "redis");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert_eq!(parsed.port(), Some(6379));
    assert_eq!(parsed.path(), "/15");
    Client::open(uri).unwrap()
}

static VALKEY_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn delete_job_with_memberships(c: &mut redis::aio::MultiplexedConnection, id: &str) {
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg("queue:*")
        .query_async(c)
        .await
        .unwrap();
    for key in keys {
        let _: usize = c.zrem(key, id).await.unwrap();
    }
    for pattern in ["processing:*", "failed:*", "cancelled:*"] {
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(pattern)
            .query_async(c)
            .await
            .unwrap();
        for key in keys {
            let _: usize = c.srem(key, id).await.unwrap();
        }
    }
    let _: usize = c.del(format!("{JOB}{id}")).await.unwrap();
}

#[tokio::test]
async fn forced_duplicate_job_id_does_not_overwrite_or_enqueue_existing_job() {
    let _guard = VALKEY_TEST_LOCK.lock().await;
    let client = test_client();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let id = ulid::Ulid::new().to_string();
    let queue = format!("queue:collision-{}", ulid::Ulid::new());
    let job_key = format!("{JOB}{id}");
    let sentinel = serde_json::json!({"id":id,"name":"sentinel","task_data":{},"status":"queued","run_at":"2020-01-01T00:00:00Z","interval":null,"created_at":null,"updated_at":null,"tags":null,"priority":0}).to_string();
    let _: () = conn.set(&job_key, &sentinel).await.unwrap();
    let enqueue_script = redis::Script::new(
        "if redis.call('EXISTS',KEYS[1])==1 then return 0 end; redis.call('SET',KEYS[1],ARGV[1]); redis.call('ZADD',KEYS[2],ARGV[2],ARGV[3]); return 1",
    );
    let committed: i32 = enqueue_script
        .key(&job_key)
        .key(&queue)
        .arg("replacement")
        .arg(100)
        .arg(&id)
        .invoke_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(committed, 0);
    let saved: String = conn.get(&job_key).await.unwrap();
    assert_eq!(saved, sentinel);
    let queued: Option<f64> = conn.zscore(&queue, &id).await.unwrap();
    assert!(queued.is_none());
    let _: usize = conn.del(job_key).await.unwrap();
}

#[tokio::test]
async fn batch_collision_is_atomic_and_failed_clear_matches_provider_membership_rules() {
    let _guard = VALKEY_TEST_LOCK.lock().await;
    let client = test_client();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let queue = format!("queue:batch-collision-{}", ulid::Ulid::new());
    let collision_id = ulid::Ulid::new().to_string();
    let sentinel = serde_json::json!({"id":collision_id,"name":"sentinel","task_data":{},"status":"queued","run_at":"2020-01-01T00:00:00Z","interval":null,"created_at":null,"updated_at":null,"tags":null,"priority":0}).to_string();
    let _: () = conn
        .set(format!("{JOB}{collision_id}"), &sentinel)
        .await
        .unwrap();

    let script = Script::new(
        "local n=tonumber(ARGV[1]); for i=1,n do if redis.call('EXISTS',ARGV[2+(i-1)*3])==1 then return 0 end end; for i=1,n do local p=2+(i-1)*3; local key=ARGV[p]; redis.call('SET',key,ARGV[p+1]); redis.call('ZADD',KEYS[1],ARGV[p+2],string.sub(key,5)) end; return 1",
    );
    let mut invoker = script.key(&queue);
    let committed: i32 = invoker
        .arg(2)
        .arg(format!("{JOB}{}", ulid::Ulid::new()))
        .arg("first")
        .arg(0)
        .arg(format!("{JOB}{collision_id}"))
        .arg("replacement")
        .arg(0)
        .invoke_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(committed, 0);
    let members: Vec<String> = conn.zrange(&queue, 0, -1).await.unwrap();
    assert!(members.is_empty());
    let saved: String = conn.get(format!("{JOB}{collision_id}")).await.unwrap();
    assert_eq!(saved, sentinel);

    let id = ulid::Ulid::new().to_string();
    let job = job_json(
        id.clone(),
        "clear-me".into(),
        serde_json::json!({}),
        None,
        0,
        "Asia/Tokyo".parse().unwrap(),
    )
    .unwrap()
    .0;
    let mut job = job;
    job.status = JobStatus::Failed;
    let _: () = conn
        .set(format!("{JOB}{id}"), serde_json::to_string(&job).unwrap())
        .await
        .unwrap();
    let failed_key = format!("{FAILED}clear-scope");
    let _: usize = conn.sadd(&failed_key, &id).await.unwrap();
    let _: usize = conn
        .zadd("queue:valkey-admin-owned", &id, score(job.priority))
        .await
        .unwrap();
    let config: RedisQueueConfig = serde_json::from_value(serde_json::json!({
        "uri":"redis://127.0.0.1:6379/15", "queues":["unused"]
    }))
    .unwrap();
    let provider = ValkeyTimezoneQueue::connect(&config, "Asia/Tokyo".parse().unwrap())
        .await
        .unwrap();
    provider
        .clear_by_status(vec![JobStatus::Failed])
        .await
        .unwrap();
    let remains: Option<String> = conn.get(format!("{JOB}{id}")).await.unwrap();
    assert!(remains.is_none());
    let _: usize = conn
        .del(vec![
            failed_key,
            "queue:valkey-admin-owned".into(),
            format!("{JOB}{collision_id}"),
        ])
        .await
        .unwrap();
}

#[tokio::test]
async fn retry_reaper_admin_and_zone_restart_preserve_job_instant() {
    let _guard = VALKEY_TEST_LOCK.lock().await;
    let client = test_client();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let queue_name = format!("lifecycle-{}", ulid::Ulid::new());
    let id = ulid::Ulid::new().to_string();
    let instant = Utc::now() - chrono::Duration::minutes(20);
    let old = instant - chrono::Duration::minutes(10);
    let mut job = Job {
        id: id.clone(),
        name: format!("class-{queue_name}"),
        data: serde_json::json!({"attempt":1}),
        status: JobStatus::Failed,
        run_at: instant,
        interval: None,
        created_at: Some(old),
        updated_at: Some(old),
        tags: None,
        priority: 4,
    };
    let original_epoch = job.run_at.timestamp_nanos_opt();
    let original_job_json = serde_json::to_string(&job).unwrap();
    let _: () = conn
        .set(format!("{JOB}{id}"), serde_json::to_string(&job).unwrap())
        .await
        .unwrap();
    let _: usize = conn
        .sadd(format!("{FAILED}{queue_name}"), &id)
        .await
        .unwrap();
    let worker = ValkeyTimezoneQueue::connect(
        &serde_json::from_value(
            serde_json::json!({"uri":"redis://127.0.0.1:6379/15","queues":[queue_name]}),
        )
        .unwrap(),
        "Asia/Tokyo".parse().unwrap(),
    )
    .await
    .unwrap();
    worker.setup().await.unwrap();
    worker.ping().await.unwrap();
    assert_eq!(worker.describe(), "redis queue");
    assert_eq!(worker.retry_failed(Some(&id)).await.unwrap(), 1);
    job.status = JobStatus::Queued;
    let failed_to_queue: Option<f64> = conn.zscore("queue:default", &id).await.unwrap();
    assert!(failed_to_queue.is_some());
    let raw: Option<String> = conn.get(format!("{JOB}{id}")).await.unwrap();
    assert!(raw.is_some(), "retry must retain the job payload");
    let raw = raw.unwrap();
    let retried: Job = serde_json::from_str(&raw).unwrap();
    assert_eq!(retried.status, JobStatus::Queued);
    assert_eq!(job.run_at.timestamp_nanos_opt(), original_epoch);
    assert_ne!(retried.run_at.timestamp(), instant.timestamp());
    let reaper_id = ulid::Ulid::new().to_string();
    let mut reaper_job = retried.clone();
    reaper_job.id = reaper_id.clone();
    reaper_job.status = JobStatus::Processing;
    reaper_job.created_at = Some(old);
    reaper_job.updated_at = Some(old);
    let _: () = conn
        .set(
            format!("{JOB}{reaper_id}"),
            serde_json::to_string(&reaper_job).unwrap(),
        )
        .await
        .unwrap();
    let _: () = conn
        .sadd(format!("{PROCESSING}{queue_name}"), &reaper_id)
        .await
        .unwrap();
    worker.requeue(&0).await.unwrap();
    let recovered: Option<String> = conn.get(format!("{JOB}{reaper_id}")).await.unwrap();
    assert!(
        recovered.is_some(),
        "processing reaper must retain the payload"
    );
    let recovered = recovered.unwrap();
    let recovered: Job = serde_json::from_str(&recovered).unwrap();
    assert_eq!(recovered.status, JobStatus::Queued);
    let terminal_id = ulid::Ulid::new().to_string();
    let mut terminal = job.clone();
    terminal.id = terminal_id.clone();
    terminal.status = JobStatus::Completed;
    terminal.created_at = Some(old - chrono::Duration::days(2));
    let _: () = conn
        .set(
            format!("{JOB}{terminal_id}"),
            serde_json::to_string(&terminal).unwrap(),
        )
        .await
        .unwrap();
    worker
        .clear_jobs_older_than(1, Some(&vec![JobStatus::Completed]))
        .await
        .unwrap();
    assert!(
        conn.get::<_, Option<String>>(format!("{JOB}{terminal_id}"))
            .await
            .unwrap()
            .is_none()
    );
    let aged_id = ulid::Ulid::new().to_string();
    let mut aged = job.clone();
    aged.id = aged_id.clone();
    aged.status = JobStatus::Failed;
    aged.created_at = Some(old - chrono::Duration::days(2));
    let _: () = conn
        .set(
            format!("{JOB}{aged_id}"),
            serde_json::to_string(&aged).unwrap(),
        )
        .await
        .unwrap();
    let aged_key = format!("{QUEUE}{queue_name}");
    let _: usize = conn
        .zadd(&aged_key, &aged_id, score(aged.priority))
        .await
        .unwrap();
    worker
        .clear_jobs_older_than(1, Some(&vec![JobStatus::Failed]))
        .await
        .unwrap();
    let aged_payload: Option<String> = conn.get(format!("{JOB}{aged_id}")).await.unwrap();
    assert!(aged_payload.is_none());
    let _: usize = conn.zrem(&aged_key, &aged_id).await.unwrap();
    let cancel_id = ulid::Ulid::new().to_string();
    let cancel_name = format!("cancel-class-{queue_name}");
    let (cancel_job, cancel_json) = job_json(
        cancel_id.clone(),
        cancel_name.clone(),
        serde_json::json!({}),
        None,
        2,
        "Asia/Tokyo".parse().unwrap(),
    )
    .unwrap();
    let _: () = conn
        .set(format!("{JOB}{cancel_id}"), cancel_json)
        .await
        .unwrap();
    let _: () = conn
        .zadd(
            format!("{QUEUE}{queue_name}"),
            &cancel_id,
            score(cancel_job.priority),
        )
        .await
        .unwrap();
    worker.cancel_jobs_by_name(&cancel_name).await.unwrap();
    let cancelled: String = conn.get(format!("{JOB}{cancel_id}")).await.unwrap();
    assert_eq!(
        serde_json::from_str::<Job>(&cancelled).unwrap().status,
        JobStatus::Cancelled
    );
    let failed_id = ulid::Ulid::new().to_string();
    let failed_job = job_json(
        failed_id.clone(),
        "failed-case".into(),
        serde_json::json!({"ok":false}),
        None,
        0,
        "Asia/Tokyo".parse().unwrap(),
    )
    .unwrap()
    .0;
    let _: () = conn
        .set(
            format!("{JOB}{failed_id}"),
            serde_json::to_string(&failed_job).unwrap(),
        )
        .await
        .unwrap();
    let _: () = conn
        .set(format!("{JOB}{failed_id}"), {
            let mut job = failed_job.clone();
            job.status = JobStatus::Failed;
            job.data["error"] = serde_json::json!("expected");
            serde_json::to_string(&job).unwrap()
        })
        .await
        .unwrap();
    let failed_raw: String = conn.get(format!("{JOB}{failed_id}")).await.unwrap();
    let failed_job: Job = serde_json::from_str(&failed_raw).unwrap();
    assert_eq!(failed_job.status, JobStatus::Failed);
    assert_eq!(failed_job.data["error"], "expected");
    let visible = worker
        .get_jobs(Some(&vec![JobStatus::Failed]), None)
        .await
        .unwrap();
    assert!(visible.iter().any(|j| j.id == failed_id));
    let clear_id = ulid::Ulid::new().to_string();
    let mut clear_job = failed_job.clone();
    clear_job.id = clear_id.clone();
    let _: () = conn
        .set(
            format!("{JOB}{clear_id}"),
            serde_json::to_string(&clear_job).unwrap(),
        )
        .await
        .unwrap();
    let _: usize = conn
        .sadd(format!("{FAILED}default"), &clear_id)
        .await
        .unwrap();
    let _: usize = conn
        .zadd("queue:default", &clear_id, score(clear_job.priority))
        .await
        .unwrap();
    let _: usize = conn
        .sadd(format!("{FAILED}default"), &failed_id)
        .await
        .unwrap();
    let _: usize = conn
        .zadd("queue:default", &failed_id, score(failed_job.priority))
        .await
        .unwrap();
    worker
        .clear_by_status(vec![JobStatus::Failed])
        .await
        .unwrap();
    let clear_remaining: Option<String> = conn.get(format!("{JOB}{clear_id}")).await.unwrap();
    assert!(clear_remaining.is_none());
    let after_failed_clear: Option<String> = conn.get(format!("{JOB}{failed_id}")).await.unwrap();
    assert!(after_failed_clear.is_none());
    delete_job_with_memberships(&mut conn, &clear_id).await;
    delete_job_with_memberships(&mut conn, &failed_id).await;
    delete_job_with_memberships(&mut conn, &cancel_id).await;
    delete_job_with_memberships(&mut conn, &reaper_id).await;
    for zone in ["Asia/Tokyo", "America/New_York"] {
        let parsed: Job = serde_json::from_str(&original_job_json).unwrap();
        assert_eq!(parsed.run_at.timestamp_nanos_opt(), original_epoch);
        let _: String = serde_json::to_string(&parsed).unwrap();
        let restart_id = ulid::Ulid::new().to_string();
        let mut restart_job = job.clone();
        restart_job.id = restart_id.clone();
        restart_job.status = JobStatus::Queued;
        restart_job.run_at = instant;
        let _: () = conn
            .set(
                format!("{JOB}{restart_id}"),
                serde_json::to_string(&restart_job).unwrap(),
            )
            .await
            .unwrap();
        let _: () = conn
            .zadd("queue:default", &restart_id, score(restart_job.priority))
            .await
            .unwrap();
        let next = ValkeyTimezoneQueue::connect(
            &serde_json::from_value(
                serde_json::json!({"uri":"redis://127.0.0.1:6379/15","queues":["default"]}),
            )
            .unwrap(),
            zone.parse().unwrap(),
        )
        .await
        .unwrap();
        assert!(
            next.get_jobs(None, None)
                .await
                .unwrap()
                .iter()
                .any(|job| job.id == restart_id)
        );
        delete_job_with_memberships(&mut conn, &restart_id).await;
    }
    let job: Job = serde_json::from_str(&raw).unwrap();
    delete_job_with_memberships(&mut conn, &job.id).await;
}

#[tokio::test]
async fn valkey_worker_missing_handler_and_shutdown_keep_processing_membership() {
    let _guard = VALKEY_TEST_LOCK.lock().await;
    let client = test_client();
    let queue_name = format!("worker-{}", ulid::Ulid::new());
    let id = ulid::Ulid::new().to_string();
    let (job, json) = job_json(
        id.clone(),
        format!("missing-{queue_name}"),
        serde_json::json!({}),
        None,
        1,
        "Asia/Tokyo".parse().unwrap(),
    )
    .unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let _: () = conn.set(format!("{JOB}{id}"), json).await.unwrap();
    let _: usize = conn
        .zadd(format!("{QUEUE}{queue_name}"), &id, score(job.priority))
        .await
        .unwrap();
    let queued: Option<f64> = conn
        .zscore(format!("{QUEUE}{queue_name}"), &id)
        .await
        .unwrap();
    assert!(queued.is_some(), "worker fixture must be queued");
    let config: RedisQueueConfig = serde_json::from_value(serde_json::json!({
        "uri":"redis://127.0.0.1:6379/15", "queues":[queue_name.clone()], "num_workers":1
    }))
    .unwrap();
    let provider = ValkeyTimezoneQueue::connect(&config, "Asia/Tokyo".parse().unwrap())
        .await
        .unwrap();
    let worker = provider.clone();
    let task = tokio::spawn(async move { worker.run(Vec::new()).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let members: Vec<String> = conn
                .smembers(format!("{PROCESSING}{queue_name}"))
                .await
                .unwrap();
            if members.contains(&id) {
                break;
            }
            let queued_ids: Vec<String> = conn
                .zrange(format!("{QUEUE}{queue_name}"), 0, -1)
                .await
                .unwrap();
            assert!(
                queued_ids.contains(&id),
                "job disappeared before worker claim: {queued_ids:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("worker should claim the queued job");
    provider.shutdown().unwrap();
    task.await.unwrap().unwrap();
    let members: Vec<String> = conn
        .smembers(format!("{PROCESSING}{queue_name}"))
        .await
        .unwrap();
    assert!(
        members.contains(&id),
        "processing membership not retained: {members:?}"
    );
    let raw: String = conn.get(format!("{JOB}{id}")).await.unwrap();
    assert_eq!(
        serde_json::from_str::<Job>(&raw).unwrap().status,
        JobStatus::Queued
    );
    delete_job_with_memberships(&mut conn, &id).await;
}

#[tokio::test]
#[ignore = "clear removes the complete Loco namespace; run only against an isolated Valkey DB"]
async fn clear_removes_queue_namespace_keys_but_preserves_unrelated_data() {
    let _guard = VALKEY_TEST_LOCK.lock().await;
    let uri = std::env::var("YORISHIRO_VALKEY_TEST_URL").unwrap();
    assert_ne!(
        reqwest::Url::parse(&uri).unwrap().path(),
        "/15",
        "global queue clear test requires a dedicated Valkey database, not reserved DB 15"
    );
    let client = Client::open(uri.clone()).unwrap();
    let unique_queue = format!("clear-test-{}", ulid::Ulid::new());
    let config: RedisQueueConfig = serde_json::from_value(serde_json::json!({
        "uri":uri, "queues":[unique_queue.clone()], "dangerously_flush":false
    }))
    .unwrap();
    let provider = ValkeyTimezoneQueue::connect(&config, "Asia/Tokyo".parse().unwrap())
        .await
        .unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let unrelated = format!("application-sentinel:{}", ulid::Ulid::new());
    let owned_id = ulid::Ulid::new().to_string();
    let _: () = conn.set(&unrelated, "preserve").await.unwrap();
    let _: () = conn
        .set(format!("{JOB}{owned_id}"), "owned-test-payload")
        .await
        .unwrap();
    let owned_queue = format!("{QUEUE}{unique_queue}");
    let _: usize = conn.zadd(&owned_queue, &owned_id, 0).await.unwrap();

    provider.clear().await.unwrap();

    let sentinel: Option<String> = conn.get(&unrelated).await.unwrap();
    assert_eq!(sentinel.as_deref(), Some("preserve"));
    let queue_exists: bool = conn.exists(&owned_queue).await.unwrap();
    assert!(!queue_exists);
    let job: Option<String> = conn.get(format!("{JOB}{owned_id}")).await.unwrap();
    assert!(job.is_none());
    let unowned_id = ulid::Ulid::new().to_string();
    let unowned_queue = format!("{QUEUE}clear-test-other-{}", ulid::Ulid::new());
    let _: () = conn
        .set(format!("{JOB}{unowned_id}"), "other-queue-job")
        .await
        .unwrap();
    let _: usize = conn.zadd(&unowned_queue, &unowned_id, 0).await.unwrap();
    provider.clear().await.unwrap();
    let still_exists: bool = conn.exists(&unowned_queue).await.unwrap();
    assert!(still_exists);
    let still_present: Option<String> = conn.get(format!("{JOB}{unowned_id}")).await.unwrap();
    assert_eq!(still_present.as_deref(), Some("other-queue-job"));
    let _: usize = conn.del(&unowned_queue).await.unwrap();
    let _: usize = conn.del(format!("{JOB}{unowned_id}")).await.unwrap();
    let _: usize = conn.del(unrelated).await.unwrap();
}
