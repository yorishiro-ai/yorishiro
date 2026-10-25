//! The startup reindex scan runs in a worker, after its provider is installed, and is coordinated between replicas.

use crate::requests::fixtures::{TenantArgs, TestTenant};
use std::sync::Arc;

use futures::future::join_all;
use loco_rs::app::AppContext;
use loco_rs::environment::Environment;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, TransactionTrait};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::queue_job_lifecycles::Entity as Lifecycles;
use yorishiro::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use yorishiro::workers::embedding_sync::WorkerClass;
use yorishiro::workers::startup_reindex;

use crate::http_stub::HttpStub;
use crate::requests::boot_request_isolated;

/// A workspace stamped with `model`, and a worker context that enqueues through the configured Loco provider.
async fn scenario(
    request: &loco_rs::TestServer,
    ctx: &AppContext,
    model: &str,
) -> (AppContext, HttpStub, Uuid) {
    let workspace = TestTenant::create(request, ctx, TenantArgs::default())
        .await
        .workspace_id;
    let server = HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(|| async { axum::Json(serde_json::json!({"data": []})) }),
    ))
    .await;
    let previous = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "test-key".into(),
        model: model.into(),
        dimensions: 768,
        send_dimensions_param: true,
    });
    yorishiro::models::entity_embeddings::reindex_workspace(&ctx.db, workspace, &[], &previous)
        .await
        .expect("stamp the previous deployed model through reindex");
    ctx.shared_store.insert(
        Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: server.url.clone(),
            api_key: "test-key".into(),
            model: "startup-test-model".into(),
            dimensions: 768,
            send_dimensions_param: true,
        })) as Arc<dyn EmbeddingProvider>,
    );
    (ctx.clone(), server, workspace)
}

async fn queued(ctx: &AppContext, workspace: Uuid) -> bool {
    Lifecycles::has_active(&ctx.db, "startup_reindex", workspace)
        .await
        .expect("find admitted reindex")
}

#[tokio::test]
async fn a_workspace_stamped_with_another_model_is_queued_for_reindex() {
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let (worker, _server, workspace) = scenario(&request, &ctx, "an-older-model").await;
        startup_reindex::run(&worker).await;
        assert!(queued(&ctx, workspace).await);
        // Queue-routing observation: the default assignment must remain shared.
        let job = Lifecycles::find()
            .filter(yorishiro::models::queue_job_lifecycles::Column::WorkspaceId.eq(workspace))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.worker_class, WorkerClass::Shared.as_db_str());
    })
    .await;
}

#[tokio::test]
async fn a_workspace_already_on_the_configured_model_is_left_alone() {
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let (worker, _server, workspace) = scenario(&request, &ctx, "startup-test-model").await;
        startup_reindex::run(&worker).await;
        assert!(!queued(&ctx, workspace).await);
    })
    .await;
}

#[tokio::test]
async fn dimensions_only_mismatch_is_queued_and_matching_stamp_is_not() {
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let workspace = TestTenant::create(&request, &ctx, TenantArgs::default())
            .await
            .workspace_id;
        let old = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: "http://unused.invalid".into(),
            api_key: "key".into(),
            model: "same-model".into(),
            dimensions: 1536,
            send_dimensions_param: false,
        });
        yorishiro::models::entity_embeddings::reindex_workspace(&ctx.db, workspace, &[], &old)
            .await
            .unwrap();
        let current: Arc<dyn EmbeddingProvider> =
            Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                base_url: "http://unused.invalid".into(),
                api_key: "key".into(),
                model: "same-model".into(),
                dimensions: 768,
                send_dimensions_param: false,
            }));
        ctx.shared_store.insert(current);
        startup_reindex::run(&ctx).await;
        assert!(
            queued(&ctx, workspace).await,
            "width-only mismatch queues reindex"
        );
    })
    .await;
}

#[tokio::test]
async fn model_change_remains_a_reindex_mismatch() {
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let (worker, _server, workspace) = scenario(&request, &ctx, "previous-model").await;
        startup_reindex::run(&worker).await;
        assert!(queued(&ctx, workspace).await);
    })
    .await;
}

/// An already-admitted startup scan wins the atomic admission without another queue delivery.
#[tokio::test]
async fn a_workspace_with_a_reindex_already_active_is_skipped() {
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let (worker, _server, workspace) = scenario(&request, &ctx, "an-older-model").await;
        yorishiro::workers::reindex::enqueue_for_class(
            &worker,
            yorishiro::workers::reindex::ReindexArgs {
                lifecycle_id: None,
                worker_class: WorkerClass::Shared,
                workspace_id: workspace,
                startup: true,
            },
        )
        .await
        .expect("record an active reindex");

        startup_reindex::run(&worker).await;
        assert_eq!(
            Lifecycles::find()
                .filter(yorishiro::models::queue_job_lifecycles::Column::WorkspaceId.eq(workspace))
                .count(&ctx.db)
                .await
                .unwrap(),
            1
        );
    })
    .await;
}

/// Test environments run the same finite startup scan as deployed workers.
#[tokio::test]
async fn test_environments_run_the_startup_scan() {
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let (worker, _server, workspace) = scenario(&request, &ctx, "an-older-model").await;
        assert!(matches!(worker.environment, Environment::Test));
        startup_reindex::run(&worker).await;
        assert!(queued(&ctx, workspace).await);
    })
    .await;
}

/// On PostgreSQL one replica scans at a time: while another holds the scan lock a starting worker skips, and once it is released the next start scans.
#[tokio::test]
async fn postgres_replicas_take_the_scan_lock_one_at_a_time() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let (worker, _server, workspace) = scenario(&request, &ctx, "an-older-model").await;

        let holder = ctx.db.begin().await.expect("begin");
        assert!(
            yorishiro::db::try_lock_for_update(&holder, "startup-reindex-scan")
                .await
                .expect("take the lock as another replica")
        );
        startup_reindex::run(&worker).await;
        assert!(
            !queued(&ctx, workspace).await,
            "a replica that cannot take the lock does not scan"
        );

        holder.commit().await.expect("release the lock");
        startup_reindex::run(&worker).await;
        assert!(queued(&ctx, workspace).await);
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
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let (worker, _server, workspace) = scenario(&request, &ctx, "an-older-model").await;
        let first = startup_reindex::run(&worker);
        let second = startup_reindex::run(&worker);
        tokio::join!(first, second);
        assert!(queued(&ctx, workspace).await);
        assert_eq!(
            Lifecycles::find()
                .filter(
                    yorishiro::models::queue_job_lifecycles::Column::JobName.eq("startup_reindex")
                )
                .filter(yorishiro::models::queue_job_lifecycles::Column::WorkspaceId.eq(workspace))
                .count(&ctx.db)
                .await
                .expect("count lifecycles"),
            1
        );
    })
    .await;
}

/// The automatic request harness must keep each SQLite parent directory alive while parallel boots open their databases.
#[tokio::test]
async fn parallel_sqlite_boots_keep_their_database_parents_alive() {
    if !crate::require_sqlite_backend() {
        return;
    }

    let boots = (0..8).map(|_| async {
        crate::requests::boot_request_isolated::<App, _, _>(|_request, ctx| async move {
            ctx.db.ping().await.expect("ping sqlite test database");
        })
        .await;
    });
    join_all(boots).await;
}
