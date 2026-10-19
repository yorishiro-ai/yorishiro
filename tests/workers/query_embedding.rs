use std::sync::Arc;

use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use sea_orm::{ActiveModelTrait, EntityTrait, TransactionTrait};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::error::YorishiroError;
use yorishiro::models::_entities::{tenant_tenants, workspace_workspaces};
use yorishiro::models::query_embedding_requests::{Entity as Requests, QueryOutcome};
use yorishiro::models::queue_job_dispatch_outbox;
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::services::embedding::EmbeddingProvider;
use yorishiro::workers::query_embedding::{QueryEmbeddingArgs, QueryEmbeddingWorker};
use yorishiro::workers::target::WorkerTarget;

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
    let dispatch_id = Uuid::now_v7();
    let deadline_at = Requests::find_by_id(request_id)
        .one(&ctx.db)
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?
        .and_then(|request| request.deadline_at)
        .unwrap_or_else(|| chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1));
    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    queue_job_dispatch_outbox::record_query_in_transaction(
        &txn,
        dispatch_id,
        deadline_at,
        serde_json::json!({
            "request_id": request_id,
            "workspace_id": workspace_id,
        }),
        QueryEmbeddingWorker::class_name(),
        Some(WorkerTarget::Shared.queue()),
        Some(vec![WorkerTarget::Shared.tag()]),
    )
    .await
    .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    txn.commit()
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    QueryEmbeddingWorker::build(ctx)
        .perform(QueryEmbeddingArgs {
            request_id,
            workspace_id,
        })
        .await
}

/// Search jobs travel to the shared target when they are enqueued, so the worker declares no tag or queue of its own.
#[test]
fn the_query_worker_declares_no_routing_of_its_own() {
    assert!(QueryEmbeddingWorker::tags().is_empty());
    assert!(QueryEmbeddingWorker::queue().is_none());
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

        let expired = Requests::open(
            &ctx.db,
            ws,
            "alpha",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
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
        let id = Requests::open(
            &ctx.db,
            ws,
            "beta",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();

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
        let id = Requests::open(
            &ctx.db,
            ws,
            "alpha",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();

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
        let id = Requests::open(
            &ctx.db,
            ws,
            "alpha",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();

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
