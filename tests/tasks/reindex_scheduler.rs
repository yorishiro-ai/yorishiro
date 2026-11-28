use loco_rs::task::Task;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter, QuerySelect, QueryTrait,
};
use yorishiro::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};

#[tokio::test]
async fn the_default_reindex_schedule_is_automatic() {
    crate::requests::boot_request_with::<yorishiro::App, _, _>(
        &[
            ("initializers.licence_key", "".into()),
            ("initializers.reindex_schedule", "0 0 3 * * *".into()),
        ],
        |_, ctx| async move {
            let scheduler = loco_rs::scheduler::Scheduler::new::<yorishiro::App>(
                ctx.config.scheduler.as_ref().unwrap(),
                &ctx.environment,
            )
            .unwrap();
            let job = &scheduler.jobs["reindex_scheduler"];
            assert_eq!(job.cron, "0 * * * * *");
            assert!(job.run_on_start);
        },
    )
    .await;
}

#[tokio::test]
async fn community_task_scans_mismatch_without_constructing_a_provider() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let mut ctx = ctx;
        configure_deployment_metadata(&mut ctx);
        let fixture = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs::default(),
        )
        .await;
        let settings = ctx
            .config
            .settings::<yorishiro::data::settings::Settings>()
            .unwrap();
        let metadata = yorishiro::services::embedding::deployment_embedding_metadata(&settings)
            .unwrap()
            .expect("the test config provides deployment metadata");
        let mismatched = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: "http://unused.invalid".into(),
            api_key: "secret".into(),
            model: "previous-model".into(),
            dimensions: metadata.dimensions,
            send_dimensions_param: false,
            query_instruction: String::new(),
            document_prefix: String::new(),
        });
        stamp_workspace(
            &ctx,
            fixture.workspace_id,
            &mismatched.model_name(),
            mismatched.dimensions(),
        )
        .await;
        let task = yorishiro::tasks::reindex_scheduler::ReindexScheduler;
        assert!(
            ctx.shared_store
                .get::<std::sync::Arc<dyn EmbeddingProvider>>()
                .is_none(),
            "scheduler scan does not require a provider"
        );
        for _ in 0..2 {
            task.run(&ctx, &loco_rs::task::Vars::default())
                .await
                .unwrap();
            let lifecycle_count = yorishiro::models::queue_job_lifecycles::Entity::find()
                .filter(
                    yorishiro::models::queue_job_lifecycles::Column::WorkspaceId
                        .eq(fixture.workspace_id),
                )
                .filter(yorishiro::models::queue_job_lifecycles::Column::JobName.eq("startup_reindex"))
                .count(&ctx.db)
                .await
                .unwrap();
            assert_eq!(
                lifecycle_count, 1,
                "the metadata-only scan enqueued the mismatch without installing a provider"
            );
            let outbox_count = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
                .filter(
                    yorishiro::models::_entities::queue_job_dispatch_outbox::Column::WorkspaceId
                        .eq(fixture.workspace_id),
                )
                .filter(
                    yorishiro::models::_entities::queue_job_dispatch_outbox::Column::Id.in_subquery(
                        yorishiro::models::queue_job_lifecycles::Entity::find()
                            .select_only()
                            .column(yorishiro::models::queue_job_lifecycles::Column::Id)
                            .filter(
                                yorishiro::models::queue_job_lifecycles::Column::JobName
                                    .eq("startup_reindex"),
                            )
                            .filter(
                                yorishiro::models::queue_job_lifecycles::Column::WorkspaceId
                                    .eq(fixture.workspace_id),
                            )
                            .into_query(),
                    ),
                )
                .count(&ctx.db)
                .await
                .unwrap();
            assert_eq!(outbox_count, 1);
        }
        assert!(
            crate::has_active_job(&ctx.db, "startup_reindex", fixture.workspace_id)
                .await
                .unwrap()
        );
    })
    .await;
}

#[tokio::test]
async fn enqueue_rejection_is_workspace_scoped_and_scan_continues() {
    use sea_orm::DbBackend;

    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let mut ctx = ctx;
        configure_deployment_metadata(&mut ctx);
        let first = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs {
                workspace_name: "a-failing-workspace".into(),
                ..Default::default()
            },
        )
        .await;
        let second_workspace = crate::requests::fixtures::create_workspace(&ctx, first.tenant_id, "z-successful-workspace").await;
        let settings = ctx.config.settings::<yorishiro::data::settings::Settings>().unwrap();
        let metadata = yorishiro::services::embedding::deployment_embedding_metadata(&settings)
            .unwrap()
            .unwrap();
        for workspace_id in [first.workspace_id, second_workspace.id] {
            stamp_workspace(&ctx, workspace_id, "previous-model", metadata.dimensions).await;
        }
        let failed = first.workspace_id;
        let succeeds = second_workspace.id;
        let sql = match ctx.db.get_database_backend() {
            DbBackend::Postgres => format!(
                "ALTER TABLE queue_job_dispatch_outbox ADD CONSTRAINT reject_one_workspace CHECK (workspace_id <> '{failed}')"
            ),
            DbBackend::Sqlite => format!(
                "CREATE TRIGGER reject_one_workspace BEFORE INSERT ON queue_job_dispatch_outbox WHEN hex(NEW.workspace_id) = '{}' BEGIN SELECT RAISE(ABORT, 'reject selected workspace'); END",
                hex::encode_upper(failed.as_bytes())
            ),
            backend => panic!("unsupported test backend {backend:?}"),
        };
        // Direct write (c): deterministic fault injection rejecting one workspace's dispatch.
        ctx.db.execute_unprepared(&sql).await.unwrap();

        let error = yorishiro::tasks::reindex_scheduler::ReindexScheduler
            .run(&ctx, &loco_rs::task::Vars::default())
            .await.expect_err("targeted outbox rejection must be returned");
        let diagnostic = error.to_string();
        assert!(diagnostic.contains(&failed.to_string()), "{diagnostic}");
        assert!(crate::has_active_job(
            &ctx.db, "startup_reindex", succeeds
        ).await.unwrap());
        assert!(!crate::has_active_job(
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
async fn all_enqueue_failures_are_aggregated_with_each_workspace_id() {
    use sea_orm::DbBackend;

    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let mut ctx = ctx;
        configure_deployment_metadata(&mut ctx);
        let first = crate::requests::fixtures::TestTenant::create(&request, &ctx, crate::requests::fixtures::TenantArgs::default()).await;
        let second_workspace = crate::requests::fixtures::create_workspace(&ctx, first.tenant_id, "second-workspace").await;
        let settings = ctx.config.settings::<yorishiro::data::settings::Settings>().unwrap();
        let metadata = yorishiro::services::embedding::deployment_embedding_metadata(&settings)
            .unwrap().unwrap();
        for workspace_id in [first.workspace_id, second_workspace.id] {
            stamp_workspace(&ctx, workspace_id, "previous-model", metadata.dimensions).await;
        }
        let fault = match ctx.db.get_database_backend() {
            DbBackend::Postgres => "ALTER TABLE queue_job_dispatch_outbox ADD CONSTRAINT reject_all_dispatches CHECK (false)",
            DbBackend::Sqlite => "CREATE TRIGGER reject_all_dispatches BEFORE INSERT ON queue_job_dispatch_outbox BEGIN SELECT RAISE(ABORT, 'reject all dispatches'); END",
            backend => panic!("unsupported test backend {backend:?}"),
        };
        // Direct write (c): deterministic fault injection rejecting every dispatch.
        ctx.db.execute_unprepared(fault).await.unwrap();
        let error = yorishiro::tasks::reindex_scheduler::ReindexScheduler
            .run(&ctx, &loco_rs::task::Vars::default())
            .await.expect_err("both workspace enqueue attempts must fail");
        let diagnostic = error.to_string();
        assert!(diagnostic.contains(&first.workspace_id.to_string()), "{diagnostic}");
        assert!(diagnostic.contains(&second_workspace.id.to_string()), "{diagnostic}");
        for workspace_id in [first.workspace_id, second_workspace.id] {
            assert!(!crate::has_active_job(
                &ctx.db, "startup_reindex", workspace_id,
            ).await.unwrap());
        }
    }).await;
}

#[tokio::test]
async fn missing_lifecycle_table_returns_workspace_lookup_error() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let mut ctx = ctx;
        configure_deployment_metadata(&mut ctx);
        let fixture = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs::default(),
        )
        .await;
        let settings = ctx
            .config
            .settings::<yorishiro::data::settings::Settings>()
            .unwrap();
        let metadata = yorishiro::services::embedding::deployment_embedding_metadata(&settings)
            .unwrap()
            .expect("the test config provides deployment metadata");
        stamp_workspace(
            &ctx,
            fixture.workspace_id,
            "previous-model",
            metadata.dimensions,
        )
        .await;
        // Direct write (c): deterministic fault injection hiding the lifecycle table.
        crate::requests::fixtures::rename_table(
            &ctx.db,
            "queue_job_lifecycles",
            "queue_job_lifecycles_hidden",
        )
        .await;
        let error = yorishiro::tasks::reindex_scheduler::ReindexScheduler
            .run(&ctx, &loco_rs::task::Vars::default())
            .await
            .expect_err("missing lifecycle table must fail lookup");
        assert!(
            error
                .to_string()
                .contains(&fixture.workspace_id.to_string()),
            "{error}"
        );
        crate::requests::fixtures::rename_table(
            &ctx.db,
            "queue_job_lifecycles_hidden",
            "queue_job_lifecycles",
        )
        .await;
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
    // Direct write (a): a stamp from an earlier deployment model, which no operation produces (the stamp operation only fills a missing one).
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

fn configure_deployment_metadata(ctx: &mut loco_rs::app::AppContext) {
    let mut value = ctx.config.settings.as_ref().unwrap().clone();
    value["embedding"]["provider"] = serde_json::json!("openai");
    value["embedding"]["base_url"] = serde_json::json!("http://unused.invalid");
    value["embedding"]["model"] = serde_json::json!("test-deployment-model");
    ctx.config.settings = Some(value);
}

#[tokio::test]
async fn the_scan_waits_after_a_failed_scheduled_reindex() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        use yorishiro::models::queue_job_lifecycles as lifecycles;
        let mut ctx = ctx;
        configure_deployment_metadata(&mut ctx);
        let fixture = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs::default(),
        )
        .await;
        stamp_workspace(&ctx, fixture.workspace_id, "previous-model", 1).await;
        let task = yorishiro::tasks::reindex_scheduler::ReindexScheduler;
        let count = || async {
            lifecycles::Entity::find()
                .filter(lifecycles::Column::WorkspaceId.eq(fixture.workspace_id))
                .filter(lifecycles::Column::JobName.eq("startup_reindex"))
                .count(&ctx.db)
                .await
                .unwrap()
        };
        task.run(&ctx, &loco_rs::task::Vars::default())
            .await
            .unwrap();
        assert_eq!(count().await, 1);
        let job = lifecycles::Entity::find()
            .filter(lifecycles::Column::WorkspaceId.eq(fixture.workspace_id))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let status = request
            .get(&format!("/api/migration-jobs/{}", job.id))
            .add_header("Authorization", format!("Bearer {}", fixture.api_key))
            .await;
        status.assert_status_ok();
        assert_eq!(status.json::<serde_json::Value>()["status"], "queued");
        // Direct write (c): the failure a worker records after its attempts, at a chosen time.
        let fail_at = |at: chrono::DateTime<chrono::Utc>| {
            let id = job.id;
            let db = ctx.db.clone();
            async move {
                lifecycles::Entity::update_many()
                    .col_expr(
                        lifecycles::Column::Status,
                        sea_orm::sea_query::Expr::value("failed"),
                    )
                    .col_expr(
                        lifecycles::Column::FailedAt,
                        sea_orm::sea_query::Expr::value(at.fixed_offset()),
                    )
                    .filter(lifecycles::Column::Id.eq(id))
                    .exec(&db)
                    .await
                    .unwrap();
            }
        };
        fail_at(chrono::Utc::now()).await;
        task.run(&ctx, &loco_rs::task::Vars::default())
            .await
            .unwrap();
        assert_eq!(
            count().await,
            1,
            "a recent failure keeps the scan from retrying"
        );
        fail_at(chrono::Utc::now() - chrono::Duration::hours(2)).await;
        task.run(&ctx, &loco_rs::task::Vars::default())
            .await
            .unwrap();
        assert_eq!(count().await, 2, "an old failure no longer holds it back");
    })
    .await;
}
