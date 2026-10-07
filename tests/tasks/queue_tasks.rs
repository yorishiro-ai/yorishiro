//! The two operational embedding tasks queue worker jobs and hold no provider.
//!
//! A task runs in a CLI process, and only a worker process loads an embedding model, so the tasks hand the work to the queue.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::task::Vars;
use sea_orm::ActiveModelTrait;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::_entities::{tenant_tenants, workspace_workspaces};
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::{entity_embeddings, entity_entities, schema_schemas};
use yorishiro::services::embedding::EmbeddingProvider;
use yorishiro::workers::dispatch::{EmbeddingSyncDispatcher, ReindexDispatcher};
use yorishiro::workers::embedding_sync::{EmbeddingSyncArgs, WorkerClass};
use yorishiro::workers::reindex::ReindexArgs;

use crate::requests::boot_request;
use crate::requests::query_worker::KeywordProvider;

#[derive(Default)]
struct Recorder {
    documents: Mutex<Vec<EmbeddingSyncArgs>>,
    reindexes: Mutex<Vec<ReindexArgs>>,
}

#[async_trait]
impl EmbeddingSyncDispatcher for Recorder {
    async fn dispatch(
        &self,
        _ctx: &AppContext,
        args: EmbeddingSyncArgs,
    ) -> loco_rs::Result<String> {
        self.documents.lock().unwrap().push(args);
        Ok("job".into())
    }
}

#[async_trait]
impl ReindexDispatcher for Recorder {
    async fn dispatch(&self, _ctx: &AppContext, args: ReindexArgs) -> loco_rs::Result<String> {
        self.reindexes.lock().unwrap().push(args);
        Ok("job".into())
    }
}

fn install(ctx: &AppContext) -> Arc<Recorder> {
    let recorder = Arc::new(Recorder::default());
    ctx.shared_store
        .insert(recorder.clone() as Arc<dyn EmbeddingSyncDispatcher>);
    ctx.shared_store
        .insert(recorder.clone() as Arc<dyn ReindexDispatcher>);
    recorder
}

async fn run(ctx: &AppContext, task: &str, workspace_id: Uuid) -> loco_rs::Result<()> {
    let vars = Vars::from_cli_args(vec![("workspace_id".to_owned(), workspace_id.to_string())]);
    loco_rs::boot::run_task::<App>(ctx, Some(&task.to_owned()), &vars).await
}

async fn workspace_with_notes(ctx: &AppContext, notes: usize) -> (Uuid, Vec<Uuid>) {
    let tenant = tenant_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("queue-tasks".into()),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .expect("insert tenant");
    let workspace = workspace_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set("main".into()),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .expect("insert workspace");
    let definition = serde_json::from_value(serde_json::json!({
        "name": "note",
        "entity_types": { "note": { "fields": {
            "title": { "type": "string", "required": true, "x-embed": true }
        } } }
    }))
    .unwrap();
    schema_schemas::create_schema(&ctx.db, tenant.id, workspace.id, definition, None, None)
        .await
        .unwrap();
    let mut ids = Vec::new();
    for index in 0..notes {
        let record = entity_entities::create(
            &ctx.db,
            workspace.id,
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
    (workspace.id, ids)
}

#[tokio::test]
async fn resync_queues_a_job_for_each_entity_without_an_embedding() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        let recorder = install(&ctx);
        let (workspace, ids) = workspace_with_notes(&ctx, 3).await;
        // One entity already has its vector, so only the other two are missing.
        let record = entity_entities::get(&ctx.db, workspace, ids[0])
            .await
            .unwrap();
        entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            workspace,
            &record,
            &KeywordProvider::new(768),
        )
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

        let mut queued: Vec<Uuid> = recorder
            .documents
            .lock()
            .unwrap()
            .iter()
            .map(|job| {
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
    boot_request::<App, _, _>(|_request, ctx| async move {
        let recorder = install(&ctx);
        let (workspace, _) = workspace_with_notes(&ctx, 1).await;

        run(&ctx, "reindex_embeddings", workspace)
            .await
            .expect("the task only queues");

        let reindexes = recorder.reindexes.lock().unwrap();
        assert_eq!(reindexes.len(), 1);
        assert_eq!(reindexes[0].workspace_id, workspace);
        assert_eq!(reindexes[0].worker_class, WorkerClass::Shared);
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
    boot_request::<App, _, _>(|_request, ctx| async move {
        let recorder = install(&ctx);
        for task in ["resync_embeddings", "reindex_embeddings"] {
            let vars =
                Vars::from_cli_args(vec![("workspace_id".to_owned(), "not-a-uuid".to_owned())]);
            let result = loco_rs::boot::run_task::<App>(&ctx, Some(&task.to_owned()), &vars).await;
            assert!(result.is_err(), "{task} must reject it");
        }
        assert!(recorder.documents.lock().unwrap().is_empty());
        assert!(recorder.reindexes.lock().unwrap().is_empty());
    })
    .await;
}
