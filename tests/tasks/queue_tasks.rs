//! The two operational embedding tasks queue worker jobs and hold no provider.
//!
//! A task runs in a CLI process, and only a worker process loads an embedding model, so the tasks hand the work to the queue.

use crate::requests::fixtures::{TenantArgs, TestTenant};
use std::sync::Arc;

use loco_rs::app::AppContext;
use loco_rs::task::Vars;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::{entity_embeddings, entity_entities, schema_schemas};
use yorishiro::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use yorishiro::workers::embedding_sync::{EmbeddingSyncArgs, WorkerClass};
use yorishiro::workers::reindex::ReindexArgs;

use crate::http_stub::HttpStub;
use crate::requests::boot_request;

// Observe durable queue payloads, not a replacement implementation of dispatch.
async fn deliveries(ctx: &AppContext, workspace: Uuid) -> Vec<serde_json::Value> {
    use yorishiro::models::_entities::queue_job_dispatch_outbox::{Column, Entity};
    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace))
        .all(&ctx.db)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.payload)
        .collect()
}

async fn run(ctx: &AppContext, task: &str, workspace_id: Uuid) -> loco_rs::Result<()> {
    let vars = Vars::from_cli_args(vec![("workspace_id".to_owned(), workspace_id.to_string())]);
    loco_rs::boot::run_task::<App>(ctx, Some(&task.to_owned()), &vars).await
}

async fn workspace_with_notes(
    request: &loco_rs::TestServer,
    ctx: &AppContext,
    notes: usize,
) -> (Uuid, Vec<Uuid>) {
    let TestTenant {
        tenant_id: tenant,
        workspace_id: workspace,
        ..
    } = TestTenant::create(request, ctx, TenantArgs::default()).await;
    let definition = serde_json::from_value(serde_json::json!({
        "name": "note",
        "entity_types": { "note": { "fields": {
            "title": { "type": "string", "required": true, "x-embed": true }
        } } }
    }))
    .unwrap();
    schema_schemas::create_schema(&ctx.db, tenant, workspace, definition, None, None)
        .await
        .unwrap();
    let mut ids = Vec::new();
    for index in 0..notes {
        let record = entity_entities::create(
            &ctx.db,
            workspace,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": format!("alpha {index}") }),
            },
            None,
        )
        .await
        .unwrap();
        ids.push(record.id);
    }
    (workspace, ids)
}

#[tokio::test]
async fn resync_queues_a_job_for_each_entity_without_an_embedding() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (workspace, ids) = workspace_with_notes(&request, &ctx, 3).await;
        // One entity already has its vector, so only the other two are missing.
        let record = entity_entities::get(&ctx.db, workspace, ids[0])
            .await
            .unwrap();
        let server = HttpStub::start(axum::Router::new().route(
            "/embeddings",
            axum::routing::post(|| async {
                axum::Json(serde_json::json!({"data": [{"index": 0, "embedding": vec![1.0; 768]}]}))
            }),
        ))
        .await;
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: server.url.clone(),
            api_key: "test-key".into(),
            model: "task-test-model".into(),
            dimensions: 768,
            send_dimensions_param: true,
        });
        entity_embeddings::sync_embedding_for_record(&ctx.db, workspace, &record, &provider)
            .await
            .unwrap();
        assert!(
            ctx.shared_store
                .get::<Arc<dyn EmbeddingProvider>>()
                .is_none(),
            "the task runs with no provider"
        );

        run(&ctx, "resync_embeddings", workspace)
            .await
            .expect("the task only queues");

        let mut queued: Vec<Uuid> = deliveries(&ctx, workspace)
            .await
            .into_iter()
            .map(|payload| {
                let job: EmbeddingSyncArgs = serde_json::from_value(payload).unwrap();
                assert_eq!(job.workspace_id, workspace);
                assert_eq!(job.worker_class, WorkerClass::Shared);
                job.entity_id
            })
            .collect();
        queued.sort();
        let mut missing = ids[1..].to_vec();
        missing.sort();
        assert_eq!(queued, missing);
    })
    .await;
}

#[tokio::test]
async fn reindex_queues_one_job_for_the_workspace() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (workspace, _) = workspace_with_notes(&request, &ctx, 1).await;

        run(&ctx, "reindex_embeddings", workspace)
            .await
            .expect("the task only queues");

        let reindexes = deliveries(&ctx, workspace).await;
        assert_eq!(reindexes.len(), 1);
        let args: ReindexArgs = serde_json::from_value(reindexes[0].clone()).unwrap();
        assert_eq!(args.workspace_id, workspace);
        assert_eq!(args.worker_class, WorkerClass::Shared);
        assert!(
            ctx.shared_store
                .get::<Arc<dyn EmbeddingProvider>>()
                .is_none()
        );
    })
    .await;
}

#[tokio::test]
async fn the_tasks_reject_a_workspace_id_that_is_not_a_uuid() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (workspace, _) = workspace_with_notes(&request, &ctx, 0).await;
        for task in ["resync_embeddings", "reindex_embeddings"] {
            let vars =
                Vars::from_cli_args(vec![("workspace_id".to_owned(), "not-a-uuid".to_owned())]);
            let result = loco_rs::boot::run_task::<App>(&ctx, Some(&task.to_owned()), &vars).await;
            assert!(result.is_err(), "{task} must reject it");
        }
        assert!(deliveries(&ctx, workspace).await.is_empty());
    })
    .await;
}
