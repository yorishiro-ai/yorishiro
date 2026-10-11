use async_trait::async_trait;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::*;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use yorishiro::edition::ee::workers::infer_fill::*;

struct RecordingDispatcher {
    args: Mutex<Vec<InferFillArgs>>,
    fail: bool,
}

#[async_trait]
impl InferFillDispatcher for RecordingDispatcher {
    async fn dispatch(&self, _ctx: &AppContext, args: InferFillArgs) -> loco_rs::Result<String> {
        self.args.lock().unwrap().push(args);
        if self.fail {
            Err(loco_rs::Error::Message("dispatch failed".into()))
        } else {
            Ok("infer-fill-job".into())
        }
    }
}

#[tokio::test]
async fn dispatches_the_original_args_and_returns_success() {
    let ctx = crate::workers::test_context().await;
    let args = InferFillArgs {
        lifecycle_id: None,
        job_id: Uuid::now_v7(),
        workspace_id: Uuid::now_v7(),
        schema_name: "note".into(),
    };
    let dispatcher = RecordingDispatcher {
        args: Mutex::new(Vec::new()),
        fail: false,
    };

    let job_id = enqueue_infer_fill_with_dispatcher(&ctx, args.clone(), &dispatcher)
        .await
        .expect("dispatch");

    assert_eq!(job_id, "infer-fill-job");
    let recorded = dispatcher.args.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].job_id, args.job_id);
    assert_eq!(recorded[0].workspace_id, args.workspace_id);
    assert_eq!(recorded[0].schema_name, args.schema_name);
}

#[tokio::test]
async fn preserves_dispatch_failure() {
    let ctx = crate::workers::test_context().await;
    let dispatcher = RecordingDispatcher {
        args: Mutex::new(Vec::new()),
        fail: true,
    };

    let error = enqueue_infer_fill_with_dispatcher(
        &ctx,
        InferFillArgs {
            lifecycle_id: None,
            job_id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
            schema_name: "note".into(),
        },
        &dispatcher,
    )
    .await
    .expect_err("dispatch must fail");

    assert_eq!(error.to_string(), "dispatch failed");
}

#[tokio::test]
async fn rejects_a_missing_queue_before_creating_a_job() {
    let ctx = crate::workers::test_context().await;

    let error = enqueue_infer_fill(&ctx, Uuid::now_v7(), "note".into())
        .await
        .expect_err("missing queue must fail");

    assert_eq!(
        error.to_string(),
        "infer-fill requires a queue provider (configure queue: in the server config)"
    );
}

#[test]
fn infer_fill_worker_keeps_its_routing_tag_and_class_name() {
    assert_eq!(InferFillWorker::tags(), vec!["infer-fill".to_string()]);
    assert_eq!(InferFillWorker::class_name(), "InferFillWorker");
}

/// A chat-completions server that answers every request with one proposal after a pause, and records how many requests were in flight at once.
async fn slow_llm() -> (
    String,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind the fake LLM");
    let address = listener.local_addr().expect("fake LLM address");
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (task_flight, task_peak) = (in_flight.clone(), peak.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let (flight, peak) = (task_flight.clone(), task_peak.clone());
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                // The headers end the request line block; the body that follows is read to its declared length.
                loop {
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
                let now = flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                flight.fetch_sub(1, Ordering::SeqCst);
                let body = r#"{"choices":[{"message":{"content":"{\"summary\":\"proposed\"}"}}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    (format!("http://{address}"), in_flight, peak)
}

/// Two jobs for one workspace never run together, whichever backend: PostgreSQL holds an advisory lock, and SQLite, whose lock is a no-op, relies on the one-running-job-per-workspace index.
/// The job refused while the other runs makes no LLM call and succeeds once redelivered.
#[tokio::test]
async fn two_jobs_for_one_workspace_never_run_together() {
    use std::sync::atomic::Ordering;
    use yorishiro::edition::ee::models::inference_jobs::{self, InferenceJobStatus};
    use yorishiro::edition::ee::models::inference_proposals;

    let (base_url, in_flight, peak) = slow_llm().await;
    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        crate::requests::inference::licence(&ctx);
        let setup = crate::requests::inference::setup(&ctx).await;
        crate::requests::inference::create_entity(&request, &setup).await;
        // A second version leaves the entity on an outdated schema, which is what infer-fill walks.
        let next = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "name": "note",
                "entity_types": { "note": { "fields": {
                    "title": { "type": "string", "required": true },
                    "summary": { "type": "string" },
                    "extra": { "type": "string" }
                } } }
            }))
            .await;
        assert_eq!(next.status_code(), 201, "{:?}", next.text());
        let key = request
            .put("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "base_url": base_url,
                "model": "test-model",
                "api_key": "sk-test"
            }))
            .await;
        assert_eq!(key.status_code(), 204, "{:?}", key.text());

        let jobs = [Uuid::new_v4(), Uuid::new_v4()];
        for job_id in jobs {
            inference_jobs::create(&ctx.db, job_id, setup.workspace_id, "note")
                .await
                .expect("create job");
        }
        let args = |job_id| InferFillArgs {
            lifecycle_id: None,
            job_id,
            workspace_id: setup.workspace_id,
            schema_name: "note".into(),
        };
        let worker = InferFillWorker::build(&ctx);
        let (first, second) =
            tokio::join!(worker.perform(args(jobs[0])), worker.perform(args(jobs[1])));
        assert!(
            first.is_ok() != second.is_ok(),
            "exactly one of two overlapping runs is refused: {first:?} / {second:?}"
        );

        // The refused one is redelivered by the queue; here, once the other has finished.
        let refused = if first.is_ok() { jobs[1] } else { jobs[0] };
        worker
            .perform(args(refused))
            .await
            .expect("the redelivered job runs once the workspace is free");

        assert_eq!(peak.load(Ordering::SeqCst), 1, "LLM calls overlapped");
        assert_eq!(in_flight.load(Ordering::SeqCst), 0);
        for job_id in jobs {
            let job = inference_jobs::get(&ctx.db, job_id)
                .await
                .expect("read job")
                .expect("job exists");
            assert_eq!(job.status, InferenceJobStatus::Completed, "{job:?}");
            let proposals = inference_proposals::for_job(&ctx.db, setup.workspace_id, job_id)
                .await
                .expect("read proposals");
            assert_eq!(proposals.len(), 1, "one proposal per job: {proposals:?}");
        }
    })
    .await;
}
