//! Regression tests for the upstream defects documented in patches/loco-rs/README.md.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use loco_rs::{
    app::AppContext,
    bgworker::{BackgroundWorker, Queue, redis::create_provider},
    config::RedisQueueConfig,
    prelude::async_trait,
};
use redis::AsyncCommands;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};
use uuid::Uuid;

struct RetainedContextWorker {
    _queue: Arc<Queue>,
    _marker: Arc<()>,
}

#[async_trait]
impl BackgroundWorker<Value> for RetainedContextWorker {
    fn build(_: &AppContext) -> Self {
        panic!("test supplies a retained queue context")
    }
    async fn perform(&self, _: Value) -> loco_rs::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn sql_queue_close_releases_pool_and_retained_worker_context() {
    let dir = tempfile::tempdir().unwrap();
    let config = loco_rs::config::SqliteQueueConfig {
        uri: format!(
            "sqlite://{}?mode=rwc",
            dir.path().join("queue.sqlite3").display()
        ),
        dangerously_flush: false,
        enable_logging: false,
        max_connections: 2,
        min_connections: 1,
        connect_timeout: 5_000,
        idle_timeout: 5_000,
        poll_interval_sec: 1,
        num_workers: 1,
        reaper: None,
    };
    let queue = Arc::new(
        loco_rs::bgworker::sqlt::create_provider(&config)
            .await
            .unwrap(),
    );
    queue.setup().await.unwrap();
    queue.ping().await.unwrap();
    let marker = Arc::new(());
    let retained = Arc::downgrade(&marker);
    queue
        .register(RetainedContextWorker {
            _queue: queue.clone(),
            _marker: marker,
        })
        .await
        .unwrap();
    assert!(retained.upgrade().is_some());
    tokio::time::timeout(Duration::from_secs(5), queue.close())
        .await
        .unwrap()
        .unwrap();
    assert!(
        retained.upgrade().is_none(),
        "close must break the handler/context/queue ownership cycle"
    );
    assert!(
        queue.ping().await.is_err(),
        "close must await the SQL pool, not only cancel polling"
    );
}

#[derive(Clone)]
struct HeldWorker {
    attempt: Arc<AtomicUsize>,
    started: mpsc::UnboundedSender<usize>,
    release: [Arc<Notify>; 2],
    fail_first: bool,
}

#[async_trait]
impl BackgroundWorker<Value> for HeldWorker {
    fn build(_: &AppContext) -> Self {
        panic!("test constructs the worker with owned barriers")
    }

    async fn perform(&self, _: Value) -> loco_rs::Result<()> {
        let attempt = self.attempt.fetch_add(1, Ordering::SeqCst);
        self.started.send(attempt).unwrap();
        self.release[attempt].notified().await;
        if attempt == 0 && self.fail_first {
            Err(loco_rs::Error::string("first attempt failed"))
        } else {
            Ok(())
        }
    }
}

async fn run(queue: Arc<Queue>, tag: String) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move { queue.run(vec![tag]).await.unwrap() })
}

#[tokio::test]
async fn redis_claim_refreshes_age_and_fences_reaped_completion_and_failure() {
    let Some(uri) = crate::valkey_test_url() else {
        return;
    };
    let client = redis::Client::open(uri.clone()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    for fail_first in [false, true] {
        let name = format!("patch-regression:{}", Uuid::now_v7());
        let config = RedisQueueConfig {
            uri: uri.clone(),
            dangerously_flush: false,
            queues: Some(vec![name.clone()]),
            num_workers: 1,
            reaper: None,
        };
        let first = Arc::new(create_provider(&config).await.unwrap());
        let second = Arc::new(create_provider(&config).await.unwrap());
        let (started, mut starts) = mpsc::unbounded_channel();
        let probe = HeldWorker {
            attempt: Arc::new(AtomicUsize::new(0)),
            started,
            release: [Arc::new(Notify::new()), Arc::new(Notify::new())],
            fail_first,
        };
        first.register(probe.clone()).await.unwrap();
        second.register(probe.clone()).await.unwrap();
        let id = first
            .enqueue(
                HeldWorker::class_name(),
                Some(name.clone()),
                json!({"secret":"do-not-log"}),
                Some(vec![name.clone()]),
                None,
            )
            .await
            .unwrap()
            .unwrap();
        let key = format!("job:{id}");
        let mut old: Value =
            serde_json::from_str(&conn.get::<_, String>(&key).await.unwrap()).unwrap();
        old["updated_at"] = json!("2000-01-01T00:00:00Z");
        conn.set::<_, _, ()>(&key, old.to_string()).await.unwrap();
        let first_run = run(first.clone(), name.clone()).await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), starts.recv())
                .await
                .unwrap(),
            Some(0)
        );
        let current: Value =
            serde_json::from_str(&conn.get::<_, String>(&key).await.unwrap()).unwrap();
        assert_eq!(current["status"], "processing");
        assert_ne!(current["updated_at"], old["updated_at"]);
        first.requeue(&60).await.unwrap();
        assert_eq!(
            conn.zcard::<_, usize>(format!("queue:{name}"))
                .await
                .unwrap(),
            0,
            "a newly claimed old job is not stale"
        );

        // Simulate a stalled attempt, then let a second real worker claim its redelivery.
        let mut stale = current;
        stale["updated_at"] = old["updated_at"].clone();
        conn.set::<_, _, ()>(&key, stale.to_string()).await.unwrap();
        first.requeue(&60).await.unwrap();
        let second_run = run(second.clone(), name.clone()).await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), starts.recv())
                .await
                .unwrap(),
            Some(1)
        );
        let second_claim: String = conn.get(format!("claim:{id}")).await.unwrap();
        first.shutdown().unwrap();
        probe.release[0].notify_one();
        tokio::time::timeout(Duration::from_secs(10), first_run)
            .await
            .unwrap()
            .unwrap();
        let still_running: Value =
            serde_json::from_str(&conn.get::<_, String>(&key).await.unwrap()).unwrap();
        assert_eq!(
            still_running["status"], "processing",
            "old completion/failure must not overwrite the new attempt"
        );
        assert_eq!(
            conn.get::<_, String>(format!("claim:{id}")).await.unwrap(),
            second_claim
        );
        second.shutdown().unwrap();
        probe.release[1].notify_one();
        tokio::time::timeout(Duration::from_secs(10), second_run)
            .await
            .unwrap()
            .unwrap();
        let completed: Value =
            serde_json::from_str(&conn.get::<_, String>(&key).await.unwrap()).unwrap();
        assert_eq!(completed["status"], "completed");
        first.requeue(&60).await.unwrap();
        assert_eq!(
            conn.zcard::<_, usize>(format!("queue:{name}"))
                .await
                .unwrap(),
            0
        );
        first.close().await.unwrap();
        second.close().await.unwrap();
        conn.del::<_, ()>(&[
            key,
            format!("claim:{id}"),
            format!("queue:{name}"),
            format!("processing:{name}"),
        ])
        .await
        .unwrap();
    }
}
