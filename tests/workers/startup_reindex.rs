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
        query_instruction: String::new(),
        document_prefix: String::new(),
    });
    yorishiro::models::entity_embeddings::reindex_workspace(&ctx.db, workspace, &previous)
        .await
        .expect("stamp the previous deployed model through reindex");
    ctx.shared_store.insert(
        Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: server.url.clone(),
            api_key: "test-key".into(),
            model: "startup-test-model".into(),
            dimensions: 768,
            send_dimensions_param: true,
            query_instruction: String::new(),
            document_prefix: String::new(),
        })) as Arc<dyn EmbeddingProvider>,
    );
    (ctx.clone(), server, workspace)
}

async fn queued(ctx: &AppContext, workspace: Uuid) -> bool {
    crate::has_active_job(&ctx.db, "startup_reindex", workspace)
        .await
        .expect("find admitted reindex")
}

#[tokio::test]
async fn deployment_document_prefix_change_queues_and_restamps_after_publication() {
    use yorishiro::models::{
        entity_embeddings, entity_entities, schema_schemas, workspace_workspaces,
    };
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let workspace = tenant.workspace_id;
        let inputs = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
        let captured = inputs.clone();
        let server = HttpStub::start(axum::Router::new().route(
            "/embeddings",
            axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let captured = captured.clone();
                async move {
                    let text = body["input"][0].as_str().unwrap().to_owned();
                    let axis = usize::from(text.starts_with("new: "));
                    captured.lock().await.push(text);
                    let mut vector = vec![0.0_f32; 768];
                    vector[axis] = 1.0;
                    axum::Json(serde_json::json!({"data":[{"index":0,"embedding":vector}]}))
                }
            }),
        ))
        .await;
        let definition = serde_json::from_value(serde_json::json!({
            "name":"prefix-note", "entity_types":{"note":{"fields":{
                "title":{"type":"string","x-embed":true}
            }}}
        }))
        .unwrap();
        schema_schemas::create_schema(&ctx.db, tenant.tenant_id, workspace, definition, None, None)
            .await
            .unwrap();
        entity_entities::create(
            &ctx.db,
            workspace,
            entity_entities::CreateEntityInput {
                schema_name: "prefix-note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({"title":"stored document"}),
            },
            None,
        )
        .await
        .unwrap();
        let config = |prefix: &str, query: &str| OpenAiCompatibleConfig {
            base_url: server.url.clone(),
            api_key: "key".into(),
            model: "same-model".into(),
            dimensions: 768,
            send_dimensions_param: false,
            document_prefix: prefix.into(),
            query_instruction: query.into(),
        };
        let old = OpenAiCompatibleProvider::new(config("old: ", "query: "));
        let outcome = entity_embeddings::reindex_workspace(&ctx.db, workspace, &old)
            .await
            .unwrap();
        assert_eq!(outcome.reindexed, 1);
        assert!(outcome.failures.is_empty());
        let before = workspace_workspaces::Entity::find_by_id(workspace)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.embedding_document_prefix, "old: ");

        // Replacing the process provider models a worker restart with new deployment settings.
        ctx.shared_store
            .insert(Arc::new(OpenAiCompatibleProvider::new(config(
                "old: ",
                "other query: ",
            ))) as Arc<dyn EmbeddingProvider>);
        startup_reindex::run(&ctx).await;
        assert!(
            !queued(&ctx, workspace).await,
            "query-only deployment change needs no reindex"
        );
        let current = Arc::new(OpenAiCompatibleProvider::new(config(
            "new: ",
            "other query: ",
        )));
        ctx.shared_store
            .insert(current.clone() as Arc<dyn EmbeddingProvider>);
        startup_reindex::run(&ctx).await;
        assert!(queued(&ctx, workspace).await);
        let pending = workspace_workspaces::Entity::find_by_id(workspace)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            pending.embedding_document_prefix, "old: ",
            "enqueue must not publish the new stamp"
        );
        let outcome = entity_embeddings::reindex_workspace(&ctx.db, workspace, current.as_ref())
            .await
            .unwrap();
        assert_eq!(outcome.reindexed, 1);
        assert!(outcome.failures.is_empty());
        let published = workspace_workspaces::Entity::find_by_id(workspace)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(published.embedding_document_prefix, "new: ");
        assert_eq!(published.embedding_model.as_deref(), Some("same-model"));
        assert!(published.embedding_revision > before.embedding_revision);
        assert!(!published.embedding_reindexing);
        assert_eq!(
            *inputs.lock().await,
            ["old: title: stored document", "new: title: stored document"]
        );
    })
    .await;
}

#[tokio::test]
async fn a_workspace_stamped_with_another_model_is_queued_for_reindex() {
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let (worker, _server, workspace) = scenario(&request, &ctx, "an-older-model").await;
        assert!(matches!(worker.environment, Environment::Test));
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
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let workspace = tenant.workspace_id;
        let matching_workspace = {
            let transaction = ctx.db.begin().await.unwrap();
            let workspace = yorishiro::models::tenancy::create_workspace(
                &transaction,
                tenant.tenant_id,
                "matching-stamp-workspace",
                None,
                None,
            )
            .await
            .unwrap();
            transaction.commit().await.unwrap();
            workspace.id
        };
        let old = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: "http://unused.invalid".into(),
            api_key: "key".into(),
            model: "same-model".into(),
            dimensions: 1536,
            send_dimensions_param: false,
            query_instruction: String::new(),
            document_prefix: String::new(),
        });
        yorishiro::models::entity_embeddings::reindex_workspace(&ctx.db, workspace, &old)
            .await
            .unwrap();
        let current: Arc<dyn EmbeddingProvider> =
            Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                base_url: "http://unused.invalid".into(),
                api_key: "key".into(),
                model: "same-model".into(),
                dimensions: 768,
                send_dimensions_param: false,
                query_instruction: String::new(),
                document_prefix: String::new(),
            }));
        ctx.shared_store.insert(current);
        let matching = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: "http://unused.invalid".into(),
            api_key: "key".into(),
            model: "same-model".into(),
            dimensions: 768,
            send_dimensions_param: false,
            query_instruction: String::new(),
            document_prefix: String::new(),
        });
        yorishiro::models::entity_embeddings::reindex_workspace(
            &ctx.db,
            matching_workspace,
            &matching,
        )
        .await
        .unwrap();
        startup_reindex::run(&ctx).await;
        assert!(
            queued(&ctx, workspace).await,
            "width-only mismatch queues reindex"
        );
        assert!(!queued(&ctx, matching_workspace).await);
    })
    .await;
}

/// Startup repair must not duplicate an explicit reindex that is already queued.
#[tokio::test]
async fn a_workspace_with_a_reindex_already_active_is_skipped() {
    for startup in [false, true] {
        boot_request_isolated::<App, _, _>(|request, ctx| async move {
            let (worker, _server, workspace) = scenario(&request, &ctx, "an-older-model").await;
            yorishiro::workers::reindex::enqueue_for_class(
                &worker,
                yorishiro::workers::reindex::ReindexArgs {
                    lifecycle_id: None,
                    worker_class: WorkerClass::Shared,
                    workspace_id: workspace,
                    startup,
                },
            )
            .await
            .expect("record an active reindex");

            startup_reindex::run(&worker).await;
            startup_reindex::run_metadata_scan(&worker).await.unwrap();
            assert_eq!(
                Lifecycles::find()
                    .filter(
                        yorishiro::models::queue_job_lifecycles::Column::WorkspaceId.eq(workspace)
                    )
                    .count(&ctx.db)
                    .await
                    .unwrap(),
                1
            );
        })
        .await;
    }
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
