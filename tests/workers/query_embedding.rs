use std::sync::Arc;

use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use sea_orm::ActiveModelTrait;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::error::YorishiroError;
use yorishiro::models::_entities::{tenant_tenants, workspace_workspaces};
use yorishiro::models::query_embedding_requests::{Entity as Requests, QueryOutcome};
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::services::embedding::EmbeddingProvider;
use yorishiro::workers::query_embedding::{QueryEmbeddingArgs, QueryEmbeddingWorker};
use yorishiro::workers::registry::WorkerRegistry;

use crate::requests::boot_request;

async fn workspace(ctx: &AppContext) -> Uuid {
    let tenant = tenant_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("query-worker".into()),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .expect("insert tenant");
    workspace_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set("main".into()),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .expect("insert workspace")
    .id
}

async fn perform(ctx: &AppContext, workspace_id: Uuid, request_id: Uuid) -> loco_rs::Result<()> {
    QueryEmbeddingWorker::build(ctx)
        .perform(QueryEmbeddingArgs {
            request_id,
            workspace_id,
        })
        .await
}

/// The tag and queue come from the worker type itself, through the one registry Loco registration, the Redis queue list and `worker-tags` all read.
#[test]
fn the_query_worker_is_in_the_registry_with_its_own_tag_and_queue() {
    assert_eq!(QueryEmbeddingWorker::tags(), vec!["query-embedding"]);
    assert_eq!(
        QueryEmbeddingWorker::queue().as_deref(),
        Some("query-embedding")
    );
    let registry = WorkerRegistry::community();
    assert!(registry.tags().contains(&"query-embedding".to_owned()));
    assert!(registry.queues().contains(&"query-embedding".to_owned()));
    assert!(yorishiro::edition::worker_tags().contains(&"query-embedding".to_owned()));
}

/// The job names a request and a workspace and nothing else, so no query text sits in the queue.
#[test]
fn the_job_payload_carries_ids_only() {
    let args = QueryEmbeddingArgs {
        request_id: Uuid::nil(),
        workspace_id: Uuid::nil(),
    };
    let json = serde_json::to_value(&args).unwrap();
    let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["request_id", "workspace_id"]);
}

#[tokio::test]
async fn a_job_for_a_request_that_is_gone_or_finished_does_nothing() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        let ws = workspace(&ctx).await;
        let provider = Arc::new(crate::requests::query_worker::KeywordProvider::new(768));
        ctx.shared_store
            .insert(provider.clone() as Arc<dyn EmbeddingProvider>);

        perform(&ctx, ws, Uuid::now_v7())
            .await
            .expect("an unknown request is a late delivery");

        let expired = Requests::open(&ctx.db, ws, "alpha", 60).await.unwrap();
        assert!(Requests::expire(&ctx.db, ws, expired).await.unwrap());
        perform(&ctx, ws, expired)
            .await
            .expect("an expired request is a late delivery");
        assert!(
            provider.kinds.lock().unwrap().is_empty(),
            "no embedding work was done for either"
        );
        assert_eq!(
            Requests::take(&ctx.db, ws, expired).await.unwrap(),
            Some(QueryOutcome::Expired)
        );
    })
    .await;
}

#[tokio::test]
async fn the_worker_stores_a_vector_of_the_width_the_workspace_holds() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        let ws = workspace(&ctx).await;
        ctx.shared_store.insert(
            Arc::new(crate::requests::query_worker::KeywordProvider::new(768))
                as Arc<dyn EmbeddingProvider>,
        );
        let id = Requests::open(&ctx.db, ws, "beta", 60).await.unwrap();

        perform(&ctx, ws, id).await.expect("embedded");

        match Requests::take(&ctx.db, ws, id).await.unwrap() {
            Some(QueryOutcome::Ready { vector, model }) => {
                assert_eq!(vector.len(), 768);
                assert_eq!(vector[1], 1.0, "beta is the second axis");
                assert_eq!(model, "keyword-test-provider");
            }
            other => panic!("expected a stored vector, got {other:?}"),
        }
    })
    .await;
}

#[tokio::test]
async fn a_failure_is_recorded_on_the_request_and_returned_to_loco() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        let ws = workspace(&ctx).await;
        ctx.shared_store
            .insert(Arc::new(crate::requests::query_worker::FailingProvider(|| {
                YorishiroError::ProviderBusy {
                    message: "model is warming up".into(),
                    retry_after: std::time::Duration::from_secs(3),
                }
            })) as Arc<dyn EmbeddingProvider>);
        let id = Requests::open(&ctx.db, ws, "alpha", 60).await.unwrap();

        let error = perform(&ctx, ws, id)
            .await
            .expect_err("the provider failed");
        assert!(error.to_string().contains("model is warming up"), "{error}");
        match Requests::take(&ctx.db, ws, id).await.unwrap() {
            Some(QueryOutcome::Failed(message)) => assert!(message.contains("warming up")),
            other => panic!("expected a recorded failure, got {other:?}"),
        }
    })
    .await;
}

/// A worker process that never installed a provider cannot embed, and says so instead of guessing.
#[tokio::test]
async fn a_worker_without_any_provider_records_an_accurate_failure() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        let ws = workspace(&ctx).await;
        assert!(
            ctx.shared_store
                .get::<Arc<dyn EmbeddingProvider>>()
                .is_none(),
            "a booted server holds no provider"
        );
        let id = Requests::open(&ctx.db, ws, "alpha", 60).await.unwrap();

        perform(&ctx, ws, id).await.expect_err("no provider");
        match Requests::take(&ctx.db, ws, id).await.unwrap() {
            Some(QueryOutcome::Failed(message)) => {
                assert!(message.contains("no embedding provider"), "{message}");
            }
            other => panic!("expected a recorded failure, got {other:?}"),
        }
    })
    .await;
}
