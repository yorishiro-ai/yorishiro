/// What a provider swap must not change: a target keeps draining its own jobs however many another target has queued.
///
/// Redis filters tags client-side over the first 1000 entries of a queue, where the SQL providers filter in the query, so every target has to get a queue of its own on Redis.
/// Runs only where the lane's queue is the reserved Valkey database.
mod redis_routing {
    use std::sync::Arc;
    use std::time::Duration;

    use loco_rs::bgworker::{self, BackgroundWorker};
    use loco_rs::config::RedisQueueConfig;
    use std::sync::Mutex;
    use uuid::Uuid;

    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

    struct RoutingProbe;

    #[loco_rs::prelude::async_trait]
    impl BackgroundWorker<serde_json::Value> for RoutingProbe {
        fn build(_ctx: &loco_rs::app::AppContext) -> Self {
            Self
        }

        async fn perform(&self, args: serde_json::Value) -> loco_rs::Result<()> {
            SEEN.lock()
                .expect("seen tags")
                .push(args["tag"].as_str().unwrap_or_default().to_owned());
            Ok(())
        }
    }

    async fn remove_owned_jobs(uri: &str, jobs: &[(String, String)]) {
        let client = loco_rs::bgworker::redis::RedisPool::open(uri).expect("open cleanup client");
        let mut connection = client
            .get_multiplexed_async_connection()
            .await
            .expect("connect cleanup client");
        for (queue_name, id) in jobs {
            let _: usize = redis::cmd("ZREM")
                .arg(format!("queue:{queue_name}"))
                .arg(id)
                .query_async(&mut connection)
                .await
                .expect("remove owned queued member");
            let _: usize = redis::cmd("SREM")
                .arg(format!("processing:{queue_name}"))
                .arg(id)
                .query_async(&mut connection)
                .await
                .expect("remove owned processing member");
            let _: usize = redis::cmd("DEL")
                .arg(format!("job:{id}"))
                .query_async(&mut connection)
                .await
                .expect("remove owned job key");
        }
    }

    #[tokio::test]
    #[serial_test::serial(queue_backend)]
    #[serial_test::serial(process_environment)]
    async fn a_target_drains_its_own_jobs_behind_a_backlog_of_another_target() {
        let Some(uri) = crate::valkey_test_url() else {
            eprintln!(
                "skipping Valkey routing test: the lane's queue is not the reserved Valkey database"
            );
            return;
        };
        let suffix = Uuid::now_v7();
        let shared_tag = format!("worker-target:shared:test:{suffix}");
        let official_tag = format!("worker-target:official:test:{suffix}");
        let shared_queue = format!("queue-test:shared:{suffix}");
        let official_queue = format!("queue-test:official:{suffix}");
        let queue = Arc::new(
            bgworker::redis::create_provider(&RedisQueueConfig {
                uri: uri.clone(),
                dangerously_flush: false,
                queues: Some(vec![shared_queue.clone(), official_queue.clone()]),
                num_workers: 1,
                reaper: None,
            })
            .await
            .expect("the worker's Redis queue provider"),
        );
        queue.setup().await.expect("set up Redis queue");
        queue.register(RoutingProbe).await.expect("register probe");

        // More than the 1000 entries the Redis provider scans for one tag, isolated
        // on this run's two named queues so unrelated DB15 backlog cannot enter it.
        let backlog_ids = queue
            .enqueue_batch(
                RoutingProbe::class_name(),
                Some(official_queue.clone()),
                (0..1100)
                    .map(|_| {
                        (
                            serde_json::json!({ "tag": official_tag.clone() }),
                            Some(200),
                        )
                    })
                    .collect(),
                Some(vec![official_tag.clone()]),
            )
            .await
            .expect("enqueue official backlog");
        let mut owned_jobs: Vec<(String, String)> = backlog_ids
            .into_iter()
            .map(|id| (official_queue.clone(), id))
            .collect();
        let shared_id = queue
            .enqueue(
                RoutingProbe::class_name(),
                Some(shared_queue.clone()),
                serde_json::json!({ "tag": shared_tag.clone() }),
                Some(vec![shared_tag.clone()]),
                Some(100),
            )
            .await
            .expect("enqueue shared probe")
            .expect("provider returns shared job ID");
        owned_jobs.push((shared_queue, shared_id));

        let running = queue.clone();
        let worker_tag = shared_tag.clone();
        let handle = tokio::spawn(async move { running.run(vec![worker_tag]).await });
        let completed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if SEEN.lock().expect("seen tags").contains(&shared_tag) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        let _ = queue.shutdown();
        let _ = handle.await;
        remove_owned_jobs(&uri, &owned_jobs).await;
        assert!(completed.is_ok(), "the shared job was not run");
        assert_eq!(
            SEEN.lock().expect("seen tags").as_slice(),
            [shared_tag.as_str()],
            "only this target's job may run"
        );
    }
}
