//! The startup reindex scan runs in a worker, after its provider is installed, and is coordinated between replicas.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::environment::Environment;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, TransactionTrait};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::error::YorishiroError;
use yorishiro::models::_entities::{tenant_tenants, workspace_workspaces};
use yorishiro::models::queue_job_lifecycles::{Enqueue, Entity as Lifecycles};
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::services::embedding::EmbeddingProvider;
use yorishiro::workers::dispatch::ReindexDispatcher;
use yorishiro::workers::embedding_sync::{WorkerClass, WorkerClassResolver};
use yorishiro::workers::reindex::ReindexArgs;
use yorishiro::workers::startup_reindex;

use crate::requests::boot_request;
use crate::requests::query_worker::KeywordProvider;

#[derive(Default)]
struct RecordingDispatcher(Mutex<Vec<ReindexArgs>>);

#[async_trait]
impl ReindexDispatcher for RecordingDispatcher {
    async fn dispatch(&self, ctx: &AppContext, args: ReindexArgs) -> loco_rs::Result<String> {
        Lifecycles::record_enqueue(
            &ctx.db,
            Enqueue {
                id: Uuid::now_v7(),
                job_name: "reindex",
                worker_class: args.worker_class,
                workspace_id: Some(args.workspace_id),
                plan: None,
                concurrency_key: None,
                concurrency_limit: None,
            },
        )
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
        self.0.lock().unwrap().push(args);
        Ok("job".into())
    }
}

struct NoAssignment;

#[async_trait]
impl WorkerClassResolver for NoAssignment {
    async fn resolve(
        &self,
        _conn: &sea_orm::DatabaseConnection,
        _workspace_id: Uuid,
    ) -> Result<Option<WorkerClass>, YorishiroError> {
        Ok(None)
    }
}

/// A workspace stamped with `model`, and a context for a deployed process (the scan skips test environments) that records what it would enqueue.
async fn scenario(
    ctx: &AppContext,
    environment: Environment,
    model: &str,
) -> (AppContext, Arc<RecordingDispatcher>, Uuid) {
    let tenant = tenant_tenants::ActiveModel {
        name: Set("startup-reindex".into()),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .expect("insert tenant");
    let workspace = workspace_workspaces::ActiveModel {
        tenant_id: Set(tenant.id),
        name: Set("main".into()),
        status: Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        embedding_dimensions: Set(Some(768)),
        embedding_model: Set(Some(model.into())),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .expect("insert workspace");

    let dispatcher = Arc::new(RecordingDispatcher::default());
    ctx.shared_store
        .insert(Arc::new(NoAssignment) as Arc<dyn WorkerClassResolver>);
    ctx.shared_store
        .insert(dispatcher.clone() as Arc<dyn ReindexDispatcher>);
    ctx.shared_store
        .insert(Arc::new(KeywordProvider::new(768)) as Arc<dyn EmbeddingProvider>);
    let worker = AppContext::builder(environment, ctx.db.clone(), ctx.config.clone())
        .shared_store(ctx.shared_store.clone())
        .build();
    (worker, dispatcher, workspace.id)
}

fn queued(dispatcher: &RecordingDispatcher) -> Vec<Uuid> {
    dispatcher
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|args| args.workspace_id)
        .collect()
}

#[tokio::test]
async fn a_workspace_stamped_with_another_model_is_queued_for_reindex() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        let (worker, dispatcher, workspace) =
            scenario(&ctx, Environment::Production, "an-older-model").await;
        startup_reindex::run(&worker).await;
        assert_eq!(queued(&dispatcher), [workspace]);
        let job = dispatcher.0.lock().unwrap()[0].worker_class;
        assert_eq!(job, WorkerClass::Shared);
    })
    .await;
}

#[tokio::test]
async fn a_workspace_already_on_the_configured_model_is_left_alone() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        let (worker, dispatcher, _) =
            scenario(&ctx, Environment::Production, "keyword-test-provider").await;
        startup_reindex::run(&worker).await;
        assert!(queued(&dispatcher).is_empty());
    })
    .await;
}

/// Replicas that start together must not each queue the same reindex: a workspace with a reindex already queued, running or waiting is skipped, which is the whole guard on SQLite.
#[tokio::test]
async fn a_workspace_with_a_reindex_already_active_is_skipped() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        let (worker, dispatcher, workspace) =
            scenario(&ctx, Environment::Production, "an-older-model").await;
        Lifecycles::record_enqueue(
            &ctx.db,
            Enqueue {
                id: Uuid::now_v7(),
                job_name: "reindex",
                worker_class: WorkerClass::Shared,
                workspace_id: Some(workspace),
                plan: None,
                concurrency_key: None,
                concurrency_limit: None,
            },
        )
        .await
        .expect("record an active reindex");

        startup_reindex::run(&worker).await;
        assert!(queued(&dispatcher).is_empty());
    })
    .await;
}

/// A test boot never scans, because the scan would outlive the test's database.
#[tokio::test]
async fn test_environments_do_not_scan() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        let (worker, dispatcher, _) = scenario(&ctx, Environment::Test, "an-older-model").await;
        startup_reindex::run(&worker).await;
        assert!(queued(&dispatcher).is_empty());
    })
    .await;
}

/// On PostgreSQL one replica scans at a time: while another holds the scan lock a starting worker skips, and once it is released the next start scans.
#[tokio::test]
async fn postgres_replicas_take_the_scan_lock_one_at_a_time() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let (worker, dispatcher, workspace) =
            scenario(&ctx, Environment::Production, "an-older-model").await;

        let holder = ctx.db.begin().await.expect("begin");
        assert!(
            yorishiro::db::try_lock_for_update(&holder, "startup-reindex-scan")
                .await
                .expect("take the lock as another replica")
        );
        startup_reindex::run(&worker).await;
        assert!(
            queued(&dispatcher).is_empty(),
            "a replica that cannot take the lock does not scan"
        );

        holder.commit().await.expect("release the lock");
        startup_reindex::run(&worker).await;
        assert_eq!(queued(&dispatcher), [workspace]);
        crate::requests::close_app_pools(&ctx).await;
    })
    .await;
}

/// The SQLite write lock is not a process lock, so duplicate prevention must be
/// exercised at the lifecycle admission boundary rather than by a process-local mutex.
#[tokio::test]
async fn concurrent_sqlite_scans_admit_only_one_reindex() {
    if !crate::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let (worker, dispatcher, workspace) =
            scenario(&ctx, Environment::Production, "an-older-model").await;
        let first = startup_reindex::run(&worker);
        let second = startup_reindex::run(&worker);
        tokio::join!(first, second);
        assert_eq!(queued(&dispatcher), [workspace]);
        assert!(
            Lifecycles::has_active(&ctx.db, "reindex", workspace)
                .await
                .expect("check active lifecycle")
        );
    })
    .await;
}
