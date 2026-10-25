use loco_rs::{app::Hooks, task::Tasks};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter,
};
use yorishiro::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};

#[test]
fn tenant_reindex_scheduler_is_registered_in_community() {
    let mut tasks = Tasks::default();
    yorishiro::App::register_tasks(&mut tasks);
    assert!(
        tasks
            .names()
            .iter()
            .any(|name| name == "tenant_reindex_scheduler")
    );
}

#[tokio::test]
async fn community_task_scans_mismatch_without_constructing_a_provider() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let metadata = yorishiro::services::embedding::deployment_embedding_metadata(
            &ctx.config
                .settings::<yorishiro::data::settings::Settings>()
                .unwrap(),
        )
        .unwrap()
        .unwrap();
        let mismatched = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: "http://unused.invalid".into(),
            api_key: "secret".into(),
            model: "previous-model".into(),
            dimensions: metadata.dimensions,
            send_dimensions_param: false,
        });
        stamp_workspace(
            &ctx,
            fixture.workspace_id,
            &mismatched.model_name(),
            mismatched.dimensions(),
        )
        .await;
        let task = yorishiro::tasks::tenant_reindex_scheduler::TenantReindexScheduler;
        let provider = ctx
            .shared_store
            .remove::<std::sync::Arc<dyn EmbeddingProvider>>();
        assert!(
            provider.is_some(),
            "test boot installs a deployment provider"
        );
        task.run_at(&ctx, &loco_rs::task::Vars::default(), chrono::Utc::now())
            .await
            .unwrap();
        ctx.shared_store.insert(provider.unwrap());
        let count = yorishiro::models::queue_job_lifecycles::Entity::find()
            .filter(
                yorishiro::models::queue_job_lifecycles::Column::WorkspaceId
                    .eq(fixture.workspace_id),
            )
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "the metadata-only scan enqueued the mismatch without installing a provider"
        );
        task.run_at(&ctx, &loco_rs::task::Vars::default(), chrono::Utc::now())
            .await
            .unwrap();
        let repeated_count =
            yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
                .filter(
                    yorishiro::models::_entities::queue_job_dispatch_outbox::Column::WorkspaceId
                        .eq(fixture.workspace_id),
                )
                .count(&ctx.db)
                .await
                .unwrap();
        assert_eq!(repeated_count, 1);
    })
    .await;
}

#[tokio::test]
async fn enqueue_rejection_is_workspace_scoped_and_scan_continues() {
    use sea_orm::DbBackend;

    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let first = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs {
                workspace_name: "a-failing-workspace".into(),
                ..Default::default()
            },
        ).await;
        let second = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs {
                workspace_name: "z-successful-workspace".into(),
                ..Default::default()
            },
        ).await;
        let settings = ctx.config.settings::<yorishiro::data::settings::Settings>().unwrap();
        let metadata = yorishiro::services::embedding::deployment_embedding_metadata(&settings)
            .unwrap().unwrap();
        let current = yorishiro::services::embedding::deployment_embedding_metadata(&settings).unwrap().unwrap();
        assert_eq!(metadata.model, current.model);
        assert_eq!(metadata.dimensions, current.dimensions);
        for workspace_id in [first.workspace_id, second.workspace_id] {
            stamp_workspace(&ctx, workspace_id, "previous-model", metadata.dimensions).await;
        }
        let failed = first.workspace_id;
        let succeeds = second.workspace_id;
        let sql = match ctx.db.get_database_backend() {
            DbBackend::Postgres => format!(
                "ALTER TABLE queue_job_dispatch_outbox ADD CONSTRAINT reject_one_workspace CHECK (workspace_id <> '{failed}')"
            ),
            DbBackend::Sqlite => format!(
                "CREATE TRIGGER reject_one_workspace BEFORE INSERT ON queue_job_dispatch_outbox WHEN hex(NEW.workspace_id) = '{}' BEGIN SELECT RAISE(ABORT, 'reject selected workspace'); END",
                failed.as_bytes().iter().map(|byte| format!("{byte:02X}")).collect::<String>()
            ),
            backend => panic!("unsupported test backend {backend:?}"),
        };
        ctx.db.execute_unprepared(&sql).await.unwrap();

        let error = yorishiro::tasks::tenant_reindex_scheduler::TenantReindexScheduler
            .run_at(&ctx, &loco_rs::task::Vars::default(), chrono::Utc::now())
            .await.expect_err("targeted outbox rejection must be returned");
        let diagnostic = error.to_string();
        assert!(diagnostic.contains(&failed.to_string()), "{diagnostic}");
        assert!(yorishiro::models::queue_job_lifecycles::Entity::has_active(
            &ctx.db, "startup_reindex", succeeds
        ).await.unwrap());
        assert!(!yorishiro::models::queue_job_lifecycles::Entity::has_active(
            &ctx.db, "startup_reindex", failed
        ).await.unwrap());
        let good_outbox = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
            .filter(yorishiro::models::_entities::queue_job_dispatch_outbox::Column::WorkspaceId.eq(succeeds))
            .count(&ctx.db).await.unwrap();
        let bad_outbox = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
            .filter(yorishiro::models::_entities::queue_job_dispatch_outbox::Column::WorkspaceId.eq(failed))
            .count(&ctx.db).await.unwrap();
        assert_eq!(good_outbox, 1);
        assert_eq!(bad_outbox, 0);
    }).await;
}

#[tokio::test]
async fn active_lifecycle_is_a_noop_without_another_outbox_row() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let metadata = yorishiro::services::embedding::deployment_embedding_metadata(
            &ctx.config
                .settings::<yorishiro::data::settings::Settings>()
                .unwrap(),
        )
        .unwrap()
        .unwrap();
        stamp_workspace(
            &ctx,
            fixture.workspace_id,
            "previous-model",
            metadata.dimensions,
        )
        .await;
        let args = yorishiro::workers::reindex::ReindexArgs {
            lifecycle_id: None,
            workspace_id: fixture.workspace_id,
            worker_class: yorishiro::workers::embedding_sync::WorkerClass::Shared,
            startup: true,
        };
        yorishiro::workers::reindex::enqueue_for_class(&ctx, args)
            .await
            .unwrap();
        let before = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
            .filter(
                yorishiro::models::_entities::queue_job_dispatch_outbox::Column::WorkspaceId
                    .eq(fixture.workspace_id),
            )
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(before, 1);
        yorishiro::tasks::tenant_reindex_scheduler::TenantReindexScheduler
            .run_at(&ctx, &loco_rs::task::Vars::default(), chrono::Utc::now())
            .await
            .unwrap();
        let after = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
            .filter(
                yorishiro::models::_entities::queue_job_dispatch_outbox::Column::WorkspaceId
                    .eq(fixture.workspace_id),
            )
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(after, before);
        assert!(
            yorishiro::models::queue_job_lifecycles::Entity::has_active(
                &ctx.db,
                "startup_reindex",
                fixture.workspace_id,
            )
            .await
            .unwrap()
        );
    })
    .await;
}

#[tokio::test]
async fn all_enqueue_failures_are_aggregated_with_each_workspace_id() {
    use sea_orm::DbBackend;

    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let first = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let second = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let settings = ctx.config.settings::<yorishiro::data::settings::Settings>().unwrap();
        let metadata = yorishiro::services::embedding::deployment_embedding_metadata(&settings)
            .unwrap().unwrap();
        for workspace_id in [first.workspace_id, second.workspace_id] {
            stamp_workspace(&ctx, workspace_id, "previous-model", metadata.dimensions).await;
        }
        let fault = match ctx.db.get_database_backend() {
            DbBackend::Postgres => "ALTER TABLE queue_job_dispatch_outbox ADD CONSTRAINT reject_all_dispatches CHECK (false)",
            DbBackend::Sqlite => "CREATE TRIGGER reject_all_dispatches BEFORE INSERT ON queue_job_dispatch_outbox BEGIN SELECT RAISE(ABORT, 'reject all dispatches'); END",
            backend => panic!("unsupported test backend {backend:?}"),
        };
        ctx.db.execute_unprepared(fault).await.unwrap();
        let error = yorishiro::tasks::tenant_reindex_scheduler::TenantReindexScheduler
            .run_at(&ctx, &loco_rs::task::Vars::default(), chrono::Utc::now())
            .await.expect_err("both workspace enqueue attempts must fail");
        let diagnostic = error.to_string();
        assert!(diagnostic.contains(&first.workspace_id.to_string()), "{diagnostic}");
        assert!(diagnostic.contains(&second.workspace_id.to_string()), "{diagnostic}");
        for workspace_id in [first.workspace_id, second.workspace_id] {
            assert!(!yorishiro::models::queue_job_lifecycles::Entity::has_active(
                &ctx.db, "startup_reindex", workspace_id,
            ).await.unwrap());
        }
    }).await;
}

#[tokio::test]
async fn missing_lifecycle_table_returns_workspace_lookup_error() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let metadata = yorishiro::services::embedding::deployment_embedding_metadata(
            &ctx.config
                .settings::<yorishiro::data::settings::Settings>()
                .unwrap(),
        )
        .unwrap()
        .unwrap();
        stamp_workspace(
            &ctx,
            fixture.workspace_id,
            "previous-model",
            metadata.dimensions,
        )
        .await;
        ctx.db
            .execute_unprepared(
                "ALTER TABLE queue_job_lifecycles RENAME TO queue_job_lifecycles_hidden",
            )
            .await
            .unwrap();
        let error = yorishiro::tasks::tenant_reindex_scheduler::TenantReindexScheduler
            .run_at(&ctx, &loco_rs::task::Vars::default(), chrono::Utc::now())
            .await
            .expect_err("missing lifecycle table must fail lookup");
        assert!(
            error
                .to_string()
                .contains(&fixture.workspace_id.to_string()),
            "{error}"
        );
        ctx.db
            .execute_unprepared(
                "ALTER TABLE queue_job_lifecycles_hidden RENAME TO queue_job_lifecycles",
            )
            .await
            .unwrap();
    })
    .await;
}

async fn stamp_workspace(
    ctx: &loco_rs::app::AppContext,
    workspace_id: uuid::Uuid,
    model: &str,
    dimensions: usize,
) {
    use sea_orm::ActiveValue::Set;
    let workspace = yorishiro::models::workspace_workspaces::Entity::find_by_id(workspace_id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let mut active = workspace.into_active_model();
    active.embedding_model = Set(Some(model.to_owned()));
    active.embedding_dimensions = Set(Some(i32::try_from(dimensions).unwrap()));
    active.update(&ctx.db).await.unwrap();
}
